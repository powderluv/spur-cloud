use std::sync::Arc;

use sqlx::PgPool;
use tonic::transport::Channel;

use spur_proto::proto::slurm_controller_client::SlurmControllerClient;

use crate::config::Config;

/// Shared application state, injected into all route handlers via axum State.
#[derive(Clone)]
pub struct AppState {
    pub db: PgPool,
    pub spur: SlurmControllerClient<Channel>,
    /// K8s client — None when running in bare-metal mode.
    pub kube: Option<kube::Client>,
    pub config: Arc<Config>,
}
