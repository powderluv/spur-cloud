use axum::{extract::State, http::StatusCode, response::IntoResponse, Json};
use serde::{Deserialize, Serialize};

use crate::auth::jwt;
use crate::db::user_repo;
use crate::state::AppState;

#[derive(Deserialize)]
pub struct RegisterRequest {
    pub email: String,
    pub username: String,
    pub password: String,
}

#[derive(Deserialize)]
pub struct LoginRequest {
    pub email: String,
    pub password: String,
}

#[derive(Deserialize)]
pub struct RefreshRequest {
    pub token: String,
}

#[derive(Serialize)]
pub struct AuthResponse {
    pub token: String,
    pub user: UserInfo,
}

#[derive(Serialize)]
pub struct UserInfo {
    pub id: String,
    pub email: String,
    pub username: String,
    pub is_admin: bool,
}

#[derive(Serialize)]
pub struct ProvidersResponse {
    pub providers: Vec<ProviderInfo>,
}

#[derive(Serialize)]
pub struct ProviderInfo {
    pub name: String,
    pub enabled: bool,
    pub authorize_url: String,
}

pub async fn register(
    State(state): State<AppState>,
    Json(req): Json<RegisterRequest>,
) -> impl IntoResponse {
    if req.password.len() < 8 {
        return (
            StatusCode::BAD_REQUEST,
            "password must be at least 8 characters",
        )
            .into_response();
    }

    // Issue #44: Normalize email to lowercase for case-insensitive matching
    let email = normalize_email(&req.email);

    // Hash password
    use argon2::PasswordHasher;
    let salt =
        argon2::password_hash::SaltString::generate(&mut argon2::password_hash::rand_core::OsRng);
    let hash: String = match argon2::Argon2::default().hash_password(req.password.as_bytes(), &salt)
    {
        Ok(h) => h.to_string(),
        Err(_) => {
            return (StatusCode::INTERNAL_SERVER_ERROR, "password hashing failed").into_response();
        }
    };

    match user_repo::create_user(&state.db, &email, &req.username, &hash).await {
        Ok(user) => {
            let token = jwt::generate_token(
                &state.config.auth.jwt_secret,
                user.id,
                &user.email,
                &user.username,
                user.is_admin,
                state.config.auth.jwt_expiry_hours,
            )
            .unwrap();

            Json(AuthResponse {
                token,
                user: UserInfo {
                    id: user.id.to_string(),
                    email: user.email,
                    username: user.username,
                    is_admin: user.is_admin,
                },
            })
            .into_response()
        }
        Err(e) => {
            let msg = e.to_string();
            if msg.contains("duplicate") {
                (StatusCode::CONFLICT, "email or username already exists").into_response()
            } else {
                (StatusCode::INTERNAL_SERVER_ERROR, "registration failed").into_response()
            }
        }
    }
}

pub async fn login(
    State(state): State<AppState>,
    Json(req): Json<LoginRequest>,
) -> impl IntoResponse {
    use argon2::PasswordVerifier;

    // Issue #44: Normalize email to lowercase for case-insensitive lookup
    let email = normalize_email(&req.email);

    let user = match user_repo::get_user_by_email(&state.db, &email).await {
        Ok(Some(u)) => u,
        Ok(None) => {
            return (StatusCode::UNAUTHORIZED, "no account found for this email").into_response()
        }
        Err(_) => return (StatusCode::INTERNAL_SERVER_ERROR, "login failed").into_response(),
    };

    let password_hash = match &user.password_hash {
        Some(h) => h,
        None => {
            return (StatusCode::UNAUTHORIZED, "use OAuth login for this account").into_response()
        }
    };

    let parsed_hash = match argon2::PasswordHash::new(password_hash) {
        Ok(h) => h,
        Err(_) => return (StatusCode::INTERNAL_SERVER_ERROR, "auth error").into_response(),
    };

    if argon2::Argon2::default()
        .verify_password(req.password.as_bytes(), &parsed_hash)
        .is_err()
    {
        return (StatusCode::UNAUTHORIZED, "incorrect password").into_response();
    }

    let _ = user_repo::update_last_login(&state.db, user.id).await;

    let token = jwt::generate_token(
        &state.config.auth.jwt_secret,
        user.id,
        &user.email,
        &user.username,
        user.is_admin,
        state.config.auth.jwt_expiry_hours,
    )
    .unwrap();

    Json(AuthResponse {
        token,
        user: UserInfo {
            id: user.id.to_string(),
            email: user.email,
            username: user.username,
            is_admin: user.is_admin,
        },
    })
    .into_response()
}

pub async fn refresh(
    State(state): State<AppState>,
    Json(req): Json<RefreshRequest>,
) -> impl IntoResponse {
    // Verify existing token and issue a new one
    match jwt::verify_token(&state.config.auth.jwt_secret, &req.token) {
        Ok(identity) => {
            let new_token = jwt::generate_token(
                &state.config.auth.jwt_secret,
                identity.user_id,
                &identity.email,
                &identity.username,
                identity.is_admin,
                state.config.auth.jwt_expiry_hours,
            )
            .unwrap();
            Json(serde_json::json!({ "token": new_token })).into_response()
        }
        Err(_) => (StatusCode::UNAUTHORIZED, "invalid token").into_response(),
    }
}

pub async fn providers(State(state): State<AppState>) -> impl IntoResponse {
    let mut providers = vec![ProviderInfo {
        name: "local".into(),
        enabled: true,
        authorize_url: String::new(),
    }];

    if let Some(gh) = &state.config.auth.github {
        if gh.enabled {
            providers.push(ProviderInfo {
                name: "github".into(),
                enabled: true,
                authorize_url: "/api/auth/github".into(),
            });
        }
    }

    if let Some(okta) = &state.config.auth.okta {
        if okta.enabled {
            providers.push(ProviderInfo {
                name: "okta".into(),
                enabled: true,
                authorize_url: "/api/auth/okta".into(),
            });
        }
    }

    Json(ProvidersResponse { providers })
}

/// Normalize an email address: trim whitespace and lowercase.
/// Used in register and login to ensure case-insensitive email handling (#44).
pub fn normalize_email(email: &str) -> String {
    email.trim().to_lowercase()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalize_email_lowercases() {
        assert_eq!(normalize_email("User@Example.COM"), "user@example.com");
    }

    #[test]
    fn normalize_email_trims_whitespace() {
        assert_eq!(normalize_email("  user@test.com  "), "user@test.com");
    }

    #[test]
    fn normalize_email_mixed_case_amd() {
        // The actual bug from issue #44: sukesh.kalla@amd.com vs Sukesh.Kalla@amd.com
        assert_eq!(
            normalize_email("Sukesh.Kalla@amd.com"),
            normalize_email("sukesh.kalla@amd.com")
        );
    }

    #[test]
    fn normalize_email_already_lowercase() {
        assert_eq!(normalize_email("user@test.com"), "user@test.com");
    }
}
