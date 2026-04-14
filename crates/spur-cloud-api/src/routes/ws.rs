use axum::{
    extract::{Path, State, WebSocketUpgrade},
    http::StatusCode,
    response::IntoResponse,
    Extension,
};
use uuid::Uuid;

use crate::auth::jwt::Identity;
use crate::config::Backend;
use crate::db::session_repo;
use crate::state::AppState;
use crate::terminal::ws_handler;

/// GET /api/sessions/:id/terminal — upgrade to WebSocket for terminal access
pub async fn terminal_upgrade(
    State(state): State<AppState>,
    Extension(identity): Extension<Identity>,
    Path(id): Path<Uuid>,
    ws: WebSocketUpgrade,
) -> impl IntoResponse {
    // Verify session belongs to user and is running
    let session = match session_repo::get_session_for_user(&state.db, id, identity.user_id).await {
        Ok(Some(s)) => s,
        Ok(None) => return (StatusCode::NOT_FOUND, "session not found").into_response(),
        Err(_) => return (StatusCode::INTERNAL_SERVER_ERROR, "failed").into_response(),
    };

    if session.state != "running" {
        return (StatusCode::BAD_REQUEST, "session is not running").into_response();
    }

    match state.config.server.backend {
        Backend::K8s => {
            let pod_name = match &session.pod_name {
                Some(p) => p.clone(),
                None => {
                    return (StatusCode::BAD_REQUEST, "session pod not ready").into_response()
                }
            };
            let namespace = state.config.server.session_namespace.clone();
            let kube_client = state
                .kube
                .clone()
                .expect("k8s backend requires kube client");

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
