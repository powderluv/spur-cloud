use axum::{
    extract::{Path, Query, State},
    http::StatusCode,
    response::IntoResponse,
    Extension, Json,
};
use serde::Deserialize;
use tracing::{error, info};
use uuid::Uuid;

use crate::auth::principal::Principal;
use crate::config::Backend;
use crate::db::{session_repo, ssh_key_repo};
use crate::models::session::SessionDetail;
use crate::spur_client;
use crate::ssh;
use crate::state::AppState;
use spur_cloud_common::session_types::CreateSessionRequest;

#[derive(Deserialize)]
pub struct ListParams {
    pub state: Option<String>,
    #[serde(default = "default_limit")]
    pub limit: i64,
}

fn default_limit() -> i64 {
    50
}

/// POST /api/sessions — launch a new GPU session
pub async fn create_session(
    State(state): State<AppState>,
    Extension(principal): Extension<Principal>,
    Json(req): Json<CreateSessionRequest>,
) -> impl IntoResponse {
    // Validate
    if req.gpu_count < 1 || req.gpu_count > 8 {
        return (StatusCode::BAD_REQUEST, "gpu_count must be 1-8").into_response();
    }

    // Create session in DB
    let session = match session_repo::create_session(
        &state.db,
        principal.user_id,
        &req.name,
        &req.gpu_type,
        req.gpu_count,
        &req.container_image,
        req.partition.as_deref(),
        req.ssh_enabled,
        req.time_limit_min,
    )
    .await
    {
        Ok(s) => s,
        Err(e) => {
            error!("session creation failed: {e}");
            return (StatusCode::INTERNAL_SERVER_ERROR, "session creation failed").into_response();
        }
    };

    // Get SSH keys if SSH enabled
    let ssh_keys_str = if req.ssh_enabled {
        match ssh_key_repo::get_keys_for_user(&state.db, principal.user_id).await {
            Ok(keys) => keys
                .iter()
                .map(|k| k.public_key.as_str())
                .collect::<Vec<_>>()
                .join("\n"),
            Err(_) => String::new(),
        }
    } else {
        String::new()
    };

    // Compute SSH port for bare-metal mode
    let ssh_port = if req.ssh_enabled && state.config.server.backend == Backend::BareMetal {
        let bm = state.config.bare_metal.as_ref();
        Some(ssh::service_manager::ssh_port_for_session(
            &session.id,
            bm.map(|c| c.ssh_port_base).unwrap_or(10000),
            bm.map(|c| c.ssh_port_range).unwrap_or(50000),
        ))
    } else {
        None
    };

    // Submit to Spur
    let bare_metal = state.config.server.backend == Backend::BareMetal;
    let mut spur = state.spur.clone();
    match spur_client::submit_session(
        &mut spur,
        &req.name,
        &req.gpu_type,
        req.gpu_count,
        &req.container_image,
        req.partition.as_deref(),
        req.ssh_enabled,
        req.time_limit_min,
        &session.id.to_string(),
        &ssh_keys_str,
        ssh_port,
        bare_metal,
    )
    .await
    {
        Ok(job_id) => {
            let _ =
                session_repo::update_session_spur_job(&state.db, session.id, job_id as i32).await;
            info!(session_id = %session.id, job_id, "session submitted");
            let detail: SessionDetail = session.into();
            (StatusCode::CREATED, Json(detail)).into_response()
        }
        Err(e) => {
            let err_msg = format!("Spur submission failed: {e}");
            error!("{err_msg}");
            let _ = session_repo::update_session_failed(&state.db, session.id, &err_msg).await;
            (StatusCode::BAD_GATEWAY, err_msg).into_response()
        }
    }
}

/// GET /api/sessions — list user's sessions
pub async fn list_sessions(
    State(state): State<AppState>,
    Extension(principal): Extension<Principal>,
    Query(params): Query<ListParams>,
) -> impl IntoResponse {
    match session_repo::list_sessions_for_user(
        &state.db,
        principal.user_id,
        params.state.as_deref(),
        params.limit,
    )
    .await
    {
        Ok(sessions) => {
            let details: Vec<SessionDetail> = sessions.into_iter().map(|s| s.into()).collect();
            Json(details).into_response()
        }
        Err(e) => {
            error!("list sessions failed: {e}");
            (StatusCode::INTERNAL_SERVER_ERROR, "failed to list sessions").into_response()
        }
    }
}

/// GET /api/sessions/:id — get session detail
pub async fn get_session(
    State(state): State<AppState>,
    Extension(principal): Extension<Principal>,
    Path(id): Path<Uuid>,
) -> impl IntoResponse {
    match session_repo::get_session_for_user(&state.db, id, principal.user_id).await {
        Ok(Some(session)) => {
            let detail: SessionDetail = session.into();
            Json(detail).into_response()
        }
        Ok(None) => (StatusCode::NOT_FOUND, "session not found").into_response(),
        Err(e) => {
            error!("get session failed: {e}");
            (StatusCode::INTERNAL_SERVER_ERROR, "failed to get session").into_response()
        }
    }
}

/// DELETE /api/sessions/:id — cancel/terminate session
pub async fn delete_session(
    State(state): State<AppState>,
    Extension(principal): Extension<Principal>,
    Path(id): Path<Uuid>,
) -> impl IntoResponse {
    let session = match session_repo::get_session_for_user(&state.db, id, principal.user_id).await {
        Ok(Some(s)) => s,
        Ok(None) => return (StatusCode::NOT_FOUND, "session not found").into_response(),
        Err(e) => {
            error!("get session failed: {e}");
            return (StatusCode::INTERNAL_SERVER_ERROR, "failed").into_response();
        }
    };

    // Cancel in Spur if job exists
    if let Some(job_id) = session.spur_job_id {
        let mut spur = state.spur.clone();
        if let Err(e) = spur_client::cancel_job(&mut spur, job_id as u32).await {
            error!("spur cancel failed: {e}");
        }
    }

    let _ = session_repo::update_session_ended(&state.db, id, "cancelled").await;
    info!(session_id = %id, "session cancelled");
    StatusCode::NO_CONTENT.into_response()
}
