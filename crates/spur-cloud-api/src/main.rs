mod auth;
mod config;
mod db;
mod models;
mod routes;
mod spur_client;
mod ssh;
mod state;
mod terminal;

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use clap::Parser;
use sqlx::postgres::PgPoolOptions;
use tokio::net::TcpListener;
use tower_http::cors::{Any, CorsLayer};
use tower_http::trace::TraceLayer;
use tracing::{error, info, warn};

use spur_proto::proto::slurm_controller_client::SlurmControllerClient;

use config::Config;
use state::AppState;

#[derive(Parser)]
#[command(name = "spur-cloud-api", about = "Spur Cloud GPUaaS API server")]
struct Args {
    /// Configuration file path
    #[arg(short, long, default_value = "spur-cloud.toml")]
    config: PathBuf,

    /// Log level
    #[arg(long, default_value = "info")]
    log_level: String,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let args = Args::parse();

    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| args.log_level.parse().unwrap()),
        )
        .init();

    info!(
        version = env!("CARGO_PKG_VERSION"),
        "spur-cloud-api starting"
    );

    // Load config
    let config = Config::load(&args.config)?;
    let listen_addr = config.server.listen_addr.clone();
    let config = Arc::new(config);

    // Connect to PostgreSQL
    let db = PgPoolOptions::new()
        .max_connections(20)
        .connect(&config.database.url)
        .await?;
    info!("connected to database");

    // Run migrations
    db::migrations::run_migrations(&db).await?;

    // Connect to Spur controller (gRPC)
    let spur = SlurmControllerClient::connect(config.spur.controller_addr.clone()).await?;
    info!(addr = %config.spur.controller_addr, "connected to spur controller");

    // Create kube client only when using K8s backend
    let kube = match config.server.backend {
        config::Backend::K8s => {
            let client = kube::Client::try_default().await?;
            info!("connected to kubernetes");
            Some(client)
        }
        config::Backend::BareMetal => {
            info!("bare-metal backend — skipping kubernetes client init");
            None
        }
    };

    let state = AppState {
        db: db.clone(),
        spur: spur.clone(),
        kube,
        config: config.clone(),
    };

    // Start background session sync loop
    let sync_state = state.clone();
    tokio::spawn(async move {
        session_sync_loop(sync_state).await;
    });

    // Build router
    let cors = CorsLayer::new()
        .allow_origin(Any)
        .allow_methods(Any)
        .allow_headers(Any);

    let app = routes::build_router(state)
        .layer(cors)
        .layer(TraceLayer::new_for_http());

    // Start HTTP server
    let listener = TcpListener::bind(&listen_addr).await?;
    info!(addr = %listen_addr, "HTTP server listening");
    axum::serve(listener, app).await?;

    Ok(())
}

/// Background loop that syncs session states from Spur.
/// Polls every 5 seconds for active sessions and updates their state.
async fn session_sync_loop(state: AppState) {
    let mut interval = tokio::time::interval(Duration::from_secs(5));

    loop {
        interval.tick().await;

        let active = match db::session_repo::list_active_sessions(&state.db).await {
            Ok(s) => s,
            Err(e) => {
                warn!("session sync: DB query failed: {e}");
                continue;
            }
        };

        if active.is_empty() {
            continue;
        }

        for session in &active {
            let job_id = match session.spur_job_id {
                Some(id) => id as u32,
                None => {
                    // K8s mode: session has no spur_job_id yet. Poll the SpurJob
                    // CRD to see if the operator has assigned one.
                    if state.config.server.backend == config::Backend::K8s {
                        if let Some(kube_client) = state.kube.as_ref() {
                            let ns = &state.config.server.session_namespace;
                            let crd_name = format!("session-{}", &session.id.to_string()[..8]);
                            let api: kube::Api<spur_client::SpurJob> =
                                kube::Api::namespaced(kube_client.clone(), ns);
                            if let Ok(spurjob) = api.get(&crd_name).await {
                                if let Some(status) = &spurjob.status {
                                    // Sync spur_job_id from CRD status
                                    if let Some(id) = status.spur_job_id {
                                        let _ = db::session_repo::update_session_spur_job(
                                            &state.db, session.id, id as i32,
                                        )
                                        .await;
                                    }

                                    // Sync session state from CRD status
                                    let node =
                                        status.assigned_nodes.first().cloned().unwrap_or_default();
                                    match status.state.as_str() {
                                        "Running" if !node.is_empty() => {
                                            let pod_name = format!("spur-job-{}", crd_name);
                                            let _ = db::session_repo::update_session_running(
                                                &state.db, session.id, &node, &pod_name,
                                            )
                                            .await;
                                            info!(session = %session.id, node, "K8s session running");
                                        }
                                        "Completed" | "Failed" | "Cancelled" => {
                                            let final_state = status.state.to_lowercase();
                                            let _ = db::session_repo::update_session_ended(
                                                &state.db,
                                                session.id,
                                                &final_state,
                                            )
                                            .await;
                                            info!(session = %session.id, state = %final_state, "K8s session ended");
                                        }
                                        "Pending" => {
                                            let _ = db::session_repo::update_session_state(
                                                &state.db, session.id, "pending",
                                            )
                                            .await;
                                        }
                                        _ => {}
                                    }
                                }
                            }
                        }
                    }
                    continue;
                }
            };

            let mut spur = state.spur.clone();
            let job = match spur_client::get_job(&mut spur, job_id).await {
                Ok(Some(j)) => j,
                Ok(None) => {
                    // Job gone from spur — mark failed with error
                    let _ = db::session_repo::update_session_failed(
                        &state.db,
                        session.id,
                        "Job not found in Spur scheduler (may have been cancelled externally)",
                    )
                    .await;
                    continue;
                }
                Err(e) => {
                    warn!(session = %session.id, "failed to query spur: {e}");
                    continue;
                }
            };

            // Map Spur job state to session state
            let spur_state = job.state();
            let new_state = match spur_state {
                spur_proto::proto::JobState::JobPending => "pending",
                spur_proto::proto::JobState::JobRunning => "running",
                spur_proto::proto::JobState::JobCompleting => "stopping",
                spur_proto::proto::JobState::JobCompleted => "completed",
                spur_proto::proto::JobState::JobFailed => "failed",
                spur_proto::proto::JobState::JobCancelled => "cancelled",
                spur_proto::proto::JobState::JobTimeout => "failed",
                spur_proto::proto::JobState::JobNodeFail => "failed",
                _ => continue,
            };

            if new_state == session.state {
                continue;
            }

            // Update session state
            if new_state == "running" && session.state != "running" {
                let node_name = job.nodelist.clone();
                let pod_name = format!("spur-job-{}", job_id);
                let _ = db::session_repo::update_session_running(
                    &state.db, session.id, &node_name, &pod_name,
                )
                .await;

                // Create SSH service if enabled
                if session.ssh_enabled {
                    match state.config.server.backend {
                        config::Backend::K8s => {
                            let ns = &state.config.server.session_namespace;
                            match ssh::service_manager::create_ssh_service(
                                state
                                    .kube
                                    .as_ref()
                                    .expect("k8s backend requires kube client"),
                                ns,
                                &session.id.to_string(),
                                &pod_name,
                            )
                            .await
                            {
                                Ok((host, port)) => {
                                    let ssh_host = if host.is_empty() {
                                        node_name.clone()
                                    } else {
                                        host
                                    };
                                    let _ = db::session_repo::update_session_ssh(
                                        &state.db, session.id, &ssh_host, port,
                                    )
                                    .await;
                                }
                                Err(e) => {
                                    error!(session = %session.id, "SSH service creation failed: {e}");
                                }
                            }
                        }
                        config::Backend::BareMetal => {
                            let bm = state.config.bare_metal.as_ref();
                            let ssh_port = ssh::service_manager::ssh_port_for_session(
                                &session.id,
                                bm.map(|c| c.ssh_port_base).unwrap_or(10000),
                                bm.map(|c| c.ssh_port_range).unwrap_or(50000),
                            );
                            let _ = db::session_repo::update_session_ssh(
                                &state.db,
                                session.id,
                                &node_name,
                                ssh_port as i32,
                            )
                            .await;
                        }
                    }
                }

                // Record usage start
                let _ = db::billing_repo::record_usage_start(
                    &state.db,
                    session.user_id,
                    session.id,
                    &session.gpu_type,
                    session.gpu_count,
                    chrono::Utc::now(),
                )
                .await;

                info!(session = %session.id, job_id, node = %node_name, "session running");
            } else if matches!(new_state, "completed" | "failed" | "cancelled") {
                let _ =
                    db::session_repo::update_session_ended(&state.db, session.id, new_state).await;

                // Finalize usage record
                let _ =
                    db::billing_repo::record_usage_end(&state.db, session.id, chrono::Utc::now())
                        .await;

                // Clean up SSH service (only needed for K8s backend)
                if session.ssh_enabled {
                    if let config::Backend::K8s = state.config.server.backend {
                        let ns = &state.config.server.session_namespace;
                        let _ = ssh::service_manager::delete_ssh_service(
                            state
                                .kube
                                .as_ref()
                                .expect("k8s backend requires kube client"),
                            ns,
                            &session.id.to_string(),
                        )
                        .await;
                    }
                    // BareMetal: sshd dies with the job, no cleanup needed
                }

                info!(session = %session.id, new_state, "session ended");
            } else {
                let _ =
                    db::session_repo::update_session_state(&state.db, session.id, new_state).await;
            }
        }
    }
}
