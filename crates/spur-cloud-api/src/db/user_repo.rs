use sqlx::PgPool;
use uuid::Uuid;

use crate::models::user::User;

pub async fn create_user(
    pool: &PgPool,
    email: &str,
    username: &str,
    password_hash: &str,
) -> sqlx::Result<User> {
    sqlx::query_as::<_, User>(
        r#"
        INSERT INTO users (email, username, password_hash)
        VALUES ($1, $2, $3)
        RETURNING *
        "#,
    )
    .bind(email)
    .bind(username)
    .bind(password_hash)
    .fetch_one(pool)
    .await
}

pub async fn get_user_by_id(pool: &PgPool, id: Uuid) -> sqlx::Result<Option<User>> {
    sqlx::query_as::<_, User>("SELECT * FROM users WHERE id = $1")
        .bind(id)
        .fetch_optional(pool)
        .await
}

pub async fn get_user_by_email(pool: &PgPool, email: &str) -> sqlx::Result<Option<User>> {
    // Issue #44: Case-insensitive email lookup (handles pre-existing mixed-case data)
    sqlx::query_as::<_, User>("SELECT * FROM users WHERE LOWER(email) = LOWER($1)")
        .bind(email)
        .fetch_optional(pool)
        .await
}

pub async fn get_user_by_username(pool: &PgPool, username: &str) -> sqlx::Result<Option<User>> {
    sqlx::query_as::<_, User>("SELECT * FROM users WHERE username = $1")
        .bind(username)
        .fetch_optional(pool)
        .await
}

pub async fn upsert_github_user(
    pool: &PgPool,
    github_id: i64,
    email: &str,
    username: &str,
    display_name: Option<&str>,
    avatar_url: Option<&str>,
) -> sqlx::Result<User> {
    sqlx::query_as::<_, User>(
        r#"
        INSERT INTO users (github_id, email, username, display_name, avatar_url)
        VALUES ($1, $2, $3, $4, $5)
        ON CONFLICT (github_id) DO UPDATE SET
            email = EXCLUDED.email,
            display_name = EXCLUDED.display_name,
            avatar_url = EXCLUDED.avatar_url,
            last_login_at = NOW()
        RETURNING *
        "#,
    )
    .bind(github_id)
    .bind(email)
    .bind(username)
    .bind(display_name)
    .bind(avatar_url)
    .fetch_one(pool)
    .await
}

pub async fn upsert_okta_user(
    pool: &PgPool,
    okta_sub: &str,
    email: &str,
    username: &str,
    display_name: Option<&str>,
    is_admin: bool,
) -> sqlx::Result<User> {
    sqlx::query_as::<_, User>(
        r#"
        INSERT INTO users (okta_sub, email, username, display_name, is_admin)
        VALUES ($1, $2, $3, $4, $5)
        ON CONFLICT (okta_sub) DO UPDATE SET
            email = EXCLUDED.email,
            display_name = EXCLUDED.display_name,
            is_admin = EXCLUDED.is_admin,
            last_login_at = NOW()
        RETURNING *
        "#,
    )
    .bind(okta_sub)
    .bind(email)
    .bind(username)
    .bind(display_name)
    .bind(is_admin)
    .fetch_one(pool)
    .await
}

pub async fn update_last_login(pool: &PgPool, id: Uuid) -> sqlx::Result<()> {
    sqlx::query("UPDATE users SET last_login_at = NOW() WHERE id = $1")
        .bind(id)
        .execute(pool)
        .await?;
    Ok(())
}

/// Issue #36: Set per-user GPU quota. NULL = unlimited.
pub async fn set_user_gpu_quota(
    pool: &PgPool,
    user_id: Uuid,
    max_gpus: Option<i32>,
) -> sqlx::Result<()> {
    sqlx::query("UPDATE users SET max_gpus = $2 WHERE id = $1")
        .bind(user_id)
        .bind(max_gpus)
        .execute(pool)
        .await?;
    Ok(())
}

/// Issue #36: Set GPU quota by email (for admin endpoint).
pub async fn set_user_gpu_quota_by_email(
    pool: &PgPool,
    email: &str,
    max_gpus: Option<i32>,
) -> sqlx::Result<bool> {
    let result = sqlx::query("UPDATE users SET max_gpus = $2 WHERE LOWER(email) = LOWER($1)")
        .bind(email)
        .bind(max_gpus)
        .execute(pool)
        .await?;
    Ok(result.rows_affected() > 0)
}
