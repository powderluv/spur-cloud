use axum::{
    extract::{Request, State},
    http::StatusCode,
    middleware::Next,
    response::{IntoResponse, Response},
};

use crate::auth::jwt::{verify_token, Identity};
use crate::state::AppState;

/// Axum middleware that extracts and verifies JWT from Authorization header.
/// On success, inserts `Identity` into request extensions.
pub async fn auth_middleware(
    State(state): State<AppState>,
    mut request: Request,
    next: Next,
) -> Response {
    // Accept token from Authorization header or ?token= query parameter (for WebSocket)
    let auth_header = request
        .headers()
        .get("authorization")
        .and_then(|v| v.to_str().ok());

    let query_token = request
        .uri()
        .query()
        .and_then(|q| {
            q.split('&')
                .find(|p| p.starts_with("token="))
                .map(|p| p[6..].to_string())
        });

    let token = match auth_header {
        Some(h) if h.starts_with("Bearer ") => h[7..].to_string(),
        _ => match query_token {
            Some(t) => t,
            None => {
                return (StatusCode::UNAUTHORIZED, "missing or invalid authorization header")
                    .into_response();
            }
        },
    };
    let token = &token;

    match verify_token(&state.config.auth.jwt_secret, token) {
        Ok(identity) => {
            request.extensions_mut().insert(identity);
            next.run(request).await
        }
        Err(_) => (StatusCode::UNAUTHORIZED, "invalid or expired token").into_response(),
    }
}

/// Extractor for Identity from request extensions (set by auth_middleware).
pub fn get_identity(request: &Request) -> Option<&Identity> {
    request.extensions().get::<Identity>()
}
