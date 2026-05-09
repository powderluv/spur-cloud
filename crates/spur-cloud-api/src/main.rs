mod auth;
mod config;
mod db;
mod models;
mod routes;
mod spur_client;
mod ssh;
mod state;
mod terminal;
mod update;

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use clap::Parser;
use sqlx::postgres::PgPoolOptions;
use tokio::net::TcpListener;
use tower_http::cors::{Any, CorsLayer};
use tower_http::trace::TraceLayer;
use tracing::{error, info, warn};

use spur_cloud_common::session_types::SessionState;
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

    // Background update check (non-blocking)
    update::spawn_startup_check(env!("CARGO_PKG_VERSION"), &config.update);

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

/// Handle K8s-specific session state sync via SpurJob CRD.
/// Used when a session has no spur_job_id yet (operator hasn't assigned one).
async fn handle_k8s_crd_session(state: &AppState, session: &models::session::Session) {
    let Some(kube_client) = state.kube.as_ref() else {
        return;
    };

    let ns = &state.config.server.session_namespace;
    let crd_name = spur_client::crd_name_for_session(&session.id.to_string());
    let api: kube::Api<spur_client::SpurJob> = kube::Api::namespaced(kube_client.clone(), ns);

    let Ok(spurjob) = api.get(&crd_name).await else {
        return;
    };
    let Some(status) = &spurjob.status else {
        return;
    };

    // Sync spur_job_id from CRD status (only if changed)
    if let Some(id) = status.spur_job_id {
        if session.spur_job_id != Some(id as i32) {
            if let Err(e) =
                db::session_repo::update_session_spur_job(&state.db, session.id, id as i32).await
            {
                warn!(session = %session.id, "failed to update spur_job_id: {e}");
            }
        }
    }

    let node = status.assigned_nodes.first().cloned().unwrap_or_default();
    let current_state = SessionState::from_str(&session.state);
    let crd_state = SessionState::from_str(&status.state.to_lowercase());

    match crd_state {
        SessionState::Running if !node.is_empty() => {
            let Some(job_id) = status.spur_job_id else {
                warn!(session = %session.id, "CRD Running but no spur_job_id yet");
                return;
            };
            let pod_name = spur_client::pod_name_for(job_id, &node);

            // spurctld reports "Running" once the job is scheduled, but we hold the
            // session in "starting" until containers actually pass readiness so users
            // don't see a "running" status while the image is still being pulled.
            let containers_ready = match spur_client::check_pod_containers_ready(
                kube_client,
                ns,
                &pod_name,
            )
            .await
            {
                Ok(ready) => ready,
                Err(e) => {
                    warn!(session = %session.id, pod = %pod_name, "failed to check pod readiness: {e}");
                    false
                }
            };

            match (&current_state, containers_ready) {
                (SessionState::Pending, true) | (SessionState::Starting, true) => {
                    if let Err(e) =
                        transition_session_to_running(state, session, &node, &pod_name).await
                    {
                        error!(session = %session.id, "failed to transition to running: {e}");
                    }
                }
                (SessionState::Pending, false) => {
                    let _ = db::session_repo::update_session_state(
                        &state.db,
                        session.id,
                        SessionState::Starting.as_str(),
                    )
                    .await;
                    info!(session = %session.id, node, "K8s session starting (containers initializing)");
                }
                _ => {} // Starting+!ready: still waiting; other states: nothing to do
            }
        }
        s if s.is_terminal() => {
            let final_state = s.as_str();
            let _ =
                db::session_repo::update_session_ended(&state.db, session.id, final_state).await;
            info!(session = %session.id, state = %final_state, "K8s session ended");
        }
        SessionState::Pending if current_state != SessionState::Pending => {
            let _ = db::session_repo::update_session_state(
                &state.db,
                session.id,
                SessionState::Pending.as_str(),
            )
            .await;
        }
        _ => {}
    }
}

/// Check whether a K8s job's pod containers are ready.
/// Returns an error if the K8s client isn't configured or the API call fails.
async fn check_k8s_pod_readiness(
    state: &AppState,
    job_id: u32,
    node_name: &str,
) -> anyhow::Result<bool> {
    let kube_client = state
        .kube
        .as_ref()
        .ok_or_else(|| anyhow::anyhow!("K8s backend but no kube client available"))?;

    let pod_name = spur_client::pod_name_for(job_id, node_name);
    let ns = &state.config.server.session_namespace;

    spur_client::check_pod_containers_ready(kube_client, ns, &pod_name).await
}

/// Transition a session into the "running" state: persist the state change,
/// provision SSH (backend-aware), and record the billing start.
async fn transition_session_to_running(
    state: &AppState,
    session: &models::session::Session,
    node: &str,
    pod_name: &str,
) -> anyhow::Result<()> {
    db::session_repo::update_session_running(&state.db, session.id, node, pod_name).await?;

    if session.ssh_enabled {
        match state.config.server.backend {
            config::Backend::K8s => {
                let kube_client = state
                    .kube
                    .as_ref()
                    .ok_or_else(|| anyhow::anyhow!("K8s backend but no kube client available"))?;
                let ns = &state.config.server.session_namespace;
                match ssh::service_manager::create_ssh_service(
                    kube_client,
                    ns,
                    &session.id.to_string(),
                    pod_name,
                )
                .await
                {
                    Ok((host, port)) => {
                        let ssh_host = if host.is_empty() {
                            node.to_string()
                        } else {
                            host
                        };
                        db::session_repo::update_session_ssh(
                            &state.db, session.id, &ssh_host, port,
                        )
                        .await?;
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
                db::session_repo::update_session_ssh(&state.db, session.id, node, ssh_port as i32)
                    .await?;
            }
        }
    }

    db::billing_repo::record_usage_start(
        &state.db,
        session.user_id,
        session.id,
        &session.gpu_type,
        session.gpu_count,
        chrono::Utc::now(),
    )
    .await?;

    info!(session = %session.id, node, "session running");
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
                    // K8s mode: session has no spur_job_id yet. Poll the SpurJob CRD.
                    if state.config.server.backend == config::Backend::K8s {
                        handle_k8s_crd_session(&state, session).await;
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
            // Issue #40: Check exit code for terminal states to distinguish cancellation from failure
            let spur_state = job.state();
            let exit_code = job.exit_code;

            let current_state = SessionState::from_str(&session.state);

            // Match on Spur state first, then check backend type only for Running state
            let new_state = match spur_state {
                spur_proto::proto::JobState::JobPending => SessionState::Pending,
                spur_proto::proto::JobState::JobRunning => {
                    if state.config.server.backend == config::Backend::K8s {
                        match check_k8s_pod_readiness(&state, job_id, &job.nodelist).await {
                            Ok(true) => SessionState::Running,
                            // Pod not ready yet (pulling image): hold session in "starting"
                            Ok(false) if current_state == SessionState::Pending => {
                                SessionState::Starting
                            }
                            Ok(false) => current_state.clone(),
                            Err(e) => {
                                warn!(session = %session.id, job_id, "failed to check pod readiness: {e}");
                                current_state.clone()
                            }
                        }
                    } else {
                        SessionState::Running
                    }
                }
                spur_proto::proto::JobState::JobCompleting => SessionState::Stopping,
                spur_proto::proto::JobState::JobCompleted => match exit_code {
                    0 => SessionState::Completed,
                    // SIGINT, SIGTERM, SIGKILL — treat as cancellation, not failure
                    130 | 143 | 137 => SessionState::Cancelled,
                    _ => SessionState::Failed,
                },
                spur_proto::proto::JobState::JobFailed => match exit_code {
                    130 | 143 | 137 => SessionState::Cancelled,
                    _ => SessionState::Failed,
                },
                spur_proto::proto::JobState::JobCancelled => SessionState::Cancelled,
                spur_proto::proto::JobState::JobTimeout => SessionState::Failed,
                spur_proto::proto::JobState::JobNodeFail => SessionState::Failed,
                _ => continue,
            };

            if new_state.is_terminal() {
                info!(
                    session_id = %session.id,
                    spur_job_id = job_id,
                    spur_state = ?spur_state,
                    exit_code = exit_code,
                    mapped_state = new_state.as_str(),
                    "mapped terminal state"
                );
            }

            if new_state == current_state {
                continue;
            }

            if new_state == SessionState::Running {
                let node_name = job.nodelist.clone();
                // K8s pod name format includes the sanitized node; for BareMetal the
                // value is unused for SSH but is still persisted to the DB.
                let pod_name = match state.config.server.backend {
                    config::Backend::K8s => spur_client::pod_name_for(job_id, &node_name),
                    config::Backend::BareMetal => format!("spur-job-{}", job_id),
                };

                if let Err(e) =
                    transition_session_to_running(&state, session, &node_name, &pod_name).await
                {
                    error!(session = %session.id, "failed to transition to running: {e}");
                }
            } else if new_state.is_terminal() {
                let _ = db::session_repo::update_session_ended(
                    &state.db,
                    session.id,
                    new_state.as_str(),
                )
                .await;

                let _ =
                    db::billing_repo::record_usage_end(&state.db, session.id, chrono::Utc::now())
                        .await;

                // Clean up K8s SSH service (BareMetal sshd dies with the job)
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
                }

                info!(session = %session.id, new_state = %new_state.as_str(), "session ended");
            } else {
                let _ = db::session_repo::update_session_state(
                    &state.db,
                    session.id,
                    new_state.as_str(),
                )
                .await;
            }
        }
    }
}
