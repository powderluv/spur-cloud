use axum::{
    extract::{Path, State, WebSocketUpgrade},
    http::StatusCode,
    response::IntoResponse,
    Extension,
};
use k8s_openapi::api::core::v1::Pod;
use kube::api::{Api, ListParams};
use uuid::Uuid;

use crate::auth::principal::Principal;
use crate::config::Backend;
use crate::db::session_repo;
use crate::state::AppState;
use crate::terminal::ws_handler;

/// GET /api/sessions/:id/terminal — upgrade to WebSocket for terminal access
pub async fn terminal_upgrade(
    State(state): State<AppState>,
    Extension(principal): Extension<Principal>,
    Path(id): Path<Uuid>,
    ws: WebSocketUpgrade,
) -> impl IntoResponse {
    // Verify session belongs to user and is running
    let session = match session_repo::get_session_for_user(&state.db, id, principal.user_id).await {
        Ok(Some(s)) => s,
        Ok(None) => return (StatusCode::NOT_FOUND, "session not found").into_response(),
        Err(_) => return (StatusCode::INTERNAL_SERVER_ERROR, "failed").into_response(),
    };

    if session.state != "running" {
        return (StatusCode::BAD_REQUEST, "session is not running").into_response();
    }

    match state.config.server.backend {
        Backend::K8s => {
            let job_id = match session.spur_job_id {
                Some(id) => id,
                None => return (StatusCode::BAD_REQUEST, "session pod not ready").into_response(),
            };
            let namespace = state.config.server.session_namespace.clone();
            let kube_client = state
                .kube
                .clone()
                .expect("k8s backend requires kube client");

            // Look up the actual running pod by label rather than the stored pod_name,
            // because the agent appends a node suffix (e.g. spur-job-9-gpu-2) that
            // the status watcher does not capture when it writes pod_name to the DB.
            let pods: Api<Pod> = Api::namespaced(kube_client.clone(), &namespace);
            let lp = ListParams::default()
                .labels(&format!("spur.ai/job-id={}", job_id))
                .limit(1);
            let pod_name = match pods.list(&lp).await {
                Ok(list) => match list.items.into_iter().next().and_then(|p| p.metadata.name) {
                    Some(name) => name,
                    None => {
                        return (
                            StatusCode::SERVICE_UNAVAILABLE,
                            "session pod not found in cluster",
                        )
                            .into_response();
                    }
                },
                Err(e) => {
                    tracing::error!(session = %id, job_id, error = %e, "failed to look up pod by label");
                    return (StatusCode::INTERNAL_SERVER_ERROR, "failed to query pod")
                        .into_response();
                }
            };

            ws.on_upgrade(move |socket| {
                ws_handler::handle_terminal(socket, kube_client, namespace, pod_name)
            })
            .into_response()
        }
        Backend::BareMetal => {
            let job_id = match session.spur_job_id {
                Some(id) => id as u32,
                None => {
                    return (StatusCode::BAD_REQUEST, "session job not assigned").into_response()
                }
            };
            let spur = state.spur.clone();
            let agent_port = state
                .config
                .bare_metal
                .as_ref()
                .map(|c| c.agent_port)
                .unwrap_or(6818);

            ws.on_upgrade(move |socket| {
                ws_handler::handle_terminal_spur(socket, spur, job_id, agent_port)
            })
            .into_response()
        }
    }
}
