use chrono::Utc;
use sqlx::PgPool;
use uuid::Uuid;

use crate::models::session::Session;

pub async fn create_session(
    pool: &PgPool,
    user_id: Uuid,
    name: &str,
    gpu_type: &str,
    gpu_count: i32,
    container_image: &str,
    partition: Option<&str>,
    ssh_enabled: bool,
    time_limit_min: i32,
) -> sqlx::Result<Session> {
    sqlx::query_as::<_, Session>(
        r#"
        INSERT INTO sessions (user_id, name, gpu_type, gpu_count, container_image, partition, ssh_enabled, time_limit_min)
        VALUES ($1, $2, $3, $4, $5, $6, $7, $8)
        RETURNING *
        "#,
    )
    .bind(user_id)
    .bind(name)
    .bind(gpu_type)
    .bind(gpu_count)
    .bind(container_image)
    .bind(partition)
    .bind(ssh_enabled)
    .bind(time_limit_min)
    .fetch_one(pool)
    .await
}

pub async fn get_session(pool: &PgPool, id: Uuid) -> sqlx::Result<Option<Session>> {
    sqlx::query_as::<_, Session>("SELECT * FROM sessions WHERE id = $1")
        .bind(id)
        .fetch_optional(pool)
        .await
}

pub async fn get_session_for_user(
    pool: &PgPool,
    id: Uuid,
    user_id: Uuid,
) -> sqlx::Result<Option<Session>> {
    sqlx::query_as::<_, Session>("SELECT * FROM sessions WHERE id = $1 AND user_id = $2")
        .bind(id)
        .bind(user_id)
        .fetch_optional(pool)
        .await
}

pub async fn list_sessions_for_user(
    pool: &PgPool,
    user_id: Uuid,
    state_filter: Option<&str>,
    limit: i64,
) -> sqlx::Result<Vec<Session>> {
    if let Some(state) = state_filter {
        sqlx::query_as::<_, Session>(
            "SELECT * FROM sessions WHERE user_id = $1 AND state = $2 ORDER BY created_at DESC LIMIT $3",
        )
        .bind(user_id)
        .bind(state)
        .bind(limit)
        .fetch_all(pool)
        .await
    } else {
        sqlx::query_as::<_, Session>(
            "SELECT * FROM sessions WHERE user_id = $1 ORDER BY created_at DESC LIMIT $2",
        )
        .bind(user_id)
        .bind(limit)
        .fetch_all(pool)
        .await
    }
}

pub async fn list_active_sessions(pool: &PgPool) -> sqlx::Result<Vec<Session>> {
    sqlx::query_as::<_, Session>(
        "SELECT * FROM sessions WHERE state IN ('creating', 'pending', 'running', 'stopping')",
    )
    .fetch_all(pool)
    .await
}

/// Issue #36: Count total GPUs currently in use by a user (active sessions).
pub async fn count_active_gpus_for_user(pool: &PgPool, user_id: Uuid) -> sqlx::Result<i64> {
    let row: (i64,) = sqlx::query_as(
        "SELECT COALESCE(SUM(gpu_count), 0) FROM sessions WHERE user_id = $1 AND state IN ('creating', 'pending', 'running', 'stopping')",
    )
    .bind(user_id)
    .fetch_one(pool)
    .await?;
    Ok(row.0)
}

pub async fn update_session_state(pool: &PgPool, id: Uuid, state: &str) -> sqlx::Result<()> {
    sqlx::query("UPDATE sessions SET state = $2 WHERE id = $1")
        .bind(id)
        .bind(state)
        .execute(pool)
        .await?;
    Ok(())
}

pub async fn update_session_spur_job(
    pool: &PgPool,
    id: Uuid,
    spur_job_id: i32,
) -> sqlx::Result<()> {
    sqlx::query("UPDATE sessions SET spur_job_id = $2, state = 'pending' WHERE id = $1")
        .bind(id)
        .bind(spur_job_id)
        .execute(pool)
        .await?;
    Ok(())
}

pub async fn update_session_running(
    pool: &PgPool,
    id: Uuid,
    node_name: &str,
    pod_name: &str,
) -> sqlx::Result<()> {
    sqlx::query(
        "UPDATE sessions SET state = 'running', node_name = $2, pod_name = $3, started_at = $4 WHERE id = $1",
    )
    .bind(id)
    .bind(node_name)
    .bind(pod_name)
    .bind(Utc::now())
    .execute(pool)
    .await?;
    Ok(())
}

pub async fn update_session_ssh(
    pool: &PgPool,
    id: Uuid,
    ssh_host: &str,
    ssh_port: i32,
) -> sqlx::Result<()> {
    sqlx::query("UPDATE sessions SET ssh_host = $2, ssh_port = $3 WHERE id = $1")
        .bind(id)
        .bind(ssh_host)
        .bind(ssh_port)
        .execute(pool)
        .await?;
    Ok(())
}

pub async fn update_session_ended(pool: &PgPool, id: Uuid, final_state: &str) -> sqlx::Result<()> {
    sqlx::query("UPDATE sessions SET state = $2, ended_at = $3 WHERE id = $1")
        .bind(id)
        .bind(final_state)
        .bind(Utc::now())
        .execute(pool)
        .await?;
    Ok(())
}

/// Update session state to failed with an error message.
pub async fn update_session_failed(
    pool: &PgPool,
    id: Uuid,
    error_message: &str,
) -> sqlx::Result<()> {
    sqlx::query(
        "UPDATE sessions SET state = 'failed', ended_at = $2, error_message = $3 WHERE id = $1",
    )
    .bind(id)
    .bind(Utc::now())
    .bind(error_message)
    .execute(pool)
    .await?;
    Ok(())
}
