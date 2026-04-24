use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

#[derive(Debug, Clone, sqlx::FromRow)]
pub struct User {
    pub id: Uuid,
    pub email: String,
    pub username: String,
    pub password_hash: Option<String>,
    pub github_id: Option<i64>,
    pub okta_sub: Option<String>,
    pub display_name: Option<String>,
    pub avatar_url: Option<String>,
    pub spur_account: String,
    pub is_admin: bool,
    /// Per-user GPU quota. NULL = unlimited.
    pub max_gpus: Option<i32>,
    pub created_at: DateTime<Utc>,
    pub last_login_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UserProfile {
    pub id: Uuid,
    pub email: String,
    pub username: String,
    pub display_name: Option<String>,
    pub avatar_url: Option<String>,
    pub is_admin: bool,
    pub created_at: DateTime<Utc>,
}

impl From<User> for UserProfile {
    fn from(u: User) -> Self {
        Self {
            id: u.id,
            email: u.email,
            username: u.username,
            display_name: u.display_name,
            avatar_url: u.avatar_url,
            is_admin: u.is_admin,
            created_at: u.created_at,
        }
    }
}

#[derive(Debug, Clone, sqlx::FromRow, Serialize)]
pub struct SshKey {
    pub id: Uuid,
    pub user_id: Uuid,
    pub name: String,
    pub public_key: String,
    pub fingerprint: String,
    pub created_at: DateTime<Utc>,
}
