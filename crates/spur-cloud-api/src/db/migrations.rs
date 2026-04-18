use sqlx::PgPool;
use tracing::info;

/// Run database migrations. Creates tables if they don't exist.
pub async fn run_migrations(pool: &PgPool) -> anyhow::Result<()> {
    info!("running database migrations");

    sqlx::query(
        r#"
        CREATE TABLE IF NOT EXISTS users (
            id            UUID PRIMARY KEY DEFAULT gen_random_uuid(),
            email         TEXT UNIQUE NOT NULL,
            username      TEXT UNIQUE NOT NULL,
            password_hash TEXT,
            github_id     BIGINT UNIQUE,
            okta_sub      TEXT UNIQUE,
            display_name  TEXT,
            avatar_url    TEXT,
            spur_account  TEXT NOT NULL DEFAULT 'default',
            is_admin      BOOLEAN NOT NULL DEFAULT FALSE,
            created_at    TIMESTAMPTZ NOT NULL DEFAULT NOW(),
            last_login_at TIMESTAMPTZ
        )
        "#,
    )
    .execute(pool)
    .await?;

    sqlx::query(
        r#"
        CREATE TABLE IF NOT EXISTS ssh_keys (
            id          UUID PRIMARY KEY DEFAULT gen_random_uuid(),
            user_id     UUID NOT NULL REFERENCES users(id) ON DELETE CASCADE,
            name        TEXT NOT NULL,
            public_key  TEXT NOT NULL,
            fingerprint TEXT NOT NULL,
            created_at  TIMESTAMPTZ NOT NULL DEFAULT NOW(),
            UNIQUE(user_id, fingerprint)
        )
        "#,
    )
    .execute(pool)
    .await?;

    sqlx::query(
        r#"
        CREATE TABLE IF NOT EXISTS sessions (
            id              UUID PRIMARY KEY DEFAULT gen_random_uuid(),
            user_id         UUID NOT NULL REFERENCES users(id),
            name            TEXT NOT NULL,
            spur_job_id     INTEGER,
            state           TEXT NOT NULL DEFAULT 'creating',
            gpu_type        TEXT NOT NULL,
            gpu_count       INTEGER NOT NULL DEFAULT 1,
            container_image TEXT NOT NULL,
            partition       TEXT,
            ssh_enabled     BOOLEAN NOT NULL DEFAULT FALSE,
            ssh_port        INTEGER,
            ssh_host        TEXT,
            time_limit_min  INTEGER NOT NULL DEFAULT 240,
            created_at      TIMESTAMPTZ NOT NULL DEFAULT NOW(),
            started_at      TIMESTAMPTZ,
            ended_at        TIMESTAMPTZ,
            node_name       TEXT,
            pod_name        TEXT
        )
        "#,
    )
    .execute(pool)
    .await?;

    sqlx::query(
        r#"
        CREATE TABLE IF NOT EXISTS usage_records (
            id          UUID PRIMARY KEY DEFAULT gen_random_uuid(),
            user_id     UUID NOT NULL REFERENCES users(id),
            session_id  UUID NOT NULL REFERENCES sessions(id),
            gpu_type    TEXT NOT NULL,
            gpu_count   INTEGER NOT NULL,
            start_time  TIMESTAMPTZ NOT NULL,
            end_time    TIMESTAMPTZ,
            gpu_seconds BIGINT NOT NULL DEFAULT 0
        )
        "#,
    )
    .execute(pool)
    .await?;

    // Issue #12: Add error_message column for failed session diagnostics
    sqlx::query("ALTER TABLE sessions ADD COLUMN IF NOT EXISTS error_message TEXT")
        .execute(pool)
        .await?;

    info!("database migrations complete");
    Ok(())
}
