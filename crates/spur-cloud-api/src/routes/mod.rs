pub mod admin;
pub mod auth;
pub mod billing;
pub mod gpus;
pub mod health;
pub mod sessions;
pub mod users;
pub mod ws;

use axum::{
    middleware,
    routing::{delete, get, post, put},
    Router,
};

use crate::auth::middleware::auth_middleware;
use crate::state::AppState;

pub fn build_router(state: AppState) -> Router {
    let public_routes = Router::new()
        // Health
        .route("/healthz", get(health::healthz))
        .route("/readyz", get(health::readyz))
        // Auth (public)
        .route("/api/auth/register", post(auth::register))
        .route("/api/auth/login", post(auth::login))
        .route("/api/auth/refresh", post(auth::refresh))
        .route("/api/auth/providers", get(auth::providers))
        // OAuth (public redirects)
        .route(
            "/api/auth/github",
            get(crate::auth::github::github_authorize),
        )
        .route(
            "/api/auth/github/callback",
            get(crate::auth::github::github_callback),
        )
        .route("/api/auth/okta", get(crate::auth::okta::okta_authorize))
        .route(
            "/api/auth/okta/callback",
            get(crate::auth::okta::okta_callback),
        );

    let protected_routes = Router::new()
        // Sessions
        .route(
            "/api/sessions",
            get(sessions::list_sessions).post(sessions::create_session),
        )
        .route(
            "/api/sessions/:id",
            get(sessions::get_session).delete(sessions::delete_session),
        )
        // Terminal WebSocket
        .route("/api/sessions/:id/terminal", get(ws::terminal_upgrade))
        // GPUs
        .route("/api/gpus", get(gpus::get_capacity))
        // User profile
        .route("/api/users/me", get(users::get_profile))
        // SSH keys
        .route(
            "/api/users/me/ssh-keys",
            get(users::list_ssh_keys).post(users::add_ssh_key),
        )
        .route("/api/users/me/ssh-keys/:id", delete(users::delete_ssh_key))
        // Billing
        .route("/api/billing/usage", get(billing::get_usage))
        .route("/api/billing/summary", get(billing::get_summary))
        // Admin
        .route("/api/admin/users/quota", put(admin::set_user_quota))
        .layer(middleware::from_fn_with_state(
            state.clone(),
            auth_middleware,
        ));

    Router::new()
        .merge(public_routes)
        .merge(protected_routes)
        .with_state(state)
}
