use std::collections::{BTreeMap, HashMap};

use kube::api::{Api, DeleteParams, PostParams};
use kube::CustomResource;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use tonic::transport::Channel;
use tracing::{debug, info};

use spur_proto::proto::slurm_controller_client::SlurmControllerClient;
use spur_proto::proto::*;

// ── SpurJob CRD (minimal definition matching spur-k8s operator) ──

/// GPU configuration for a SpurJob.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct GpuSpec {
    pub count: u32,
    #[serde(default)]
    pub gpu_type: Option<String>,
}

impl Default for GpuSpec {
    fn default() -> Self {
        Self {
            count: 0,
            gpu_type: None,
        }
    }
}

/// SpurJob status — matches the operator's SpurJobStatus.
#[derive(Debug, Clone, Default, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct SpurJobStatus {
    #[serde(default)]
    pub state: String,
    #[serde(default)]
    pub spur_job_id: Option<u32>,
    #[serde(default)]
    pub assigned_nodes: Vec<String>,
}

/// SpurJob CRD spec — matches the operator's SpurJobSpec.
#[derive(CustomResource, Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[kube(
    group = "spur.ai",
    version = "v1alpha1",
    kind = "SpurJob",
    namespaced,
    status = "SpurJobStatus"
)]
#[serde(rename_all = "camelCase")]
pub struct SpurJobSpec {
    pub name: String,
    pub image: String,
    #[serde(default)]
    pub gpus: GpuSpec,
    #[serde(default = "default_one")]
    pub num_nodes: u32,
    #[serde(default = "default_one")]
    pub tasks_per_node: u32,
    #[serde(default = "default_one")]
    pub cpus_per_task: u32,
    #[serde(default)]
    pub time_limit: Option<String>,
    #[serde(default)]
    pub command: Vec<String>,
    #[serde(default)]
    pub args: Vec<String>,
    #[serde(default)]
    pub env: HashMap<String, String>,
    #[serde(default)]
    pub host_network: bool,
    #[serde(default)]
    pub privileged: bool,
}

fn default_one() -> u32 {
    1
}

/// Create a SpurJob CRD in Kubernetes for a spur-cloud session.
///
/// Used when `backend = "k8s"`. The spur-k8s operator watches for SpurJob
/// resources, submits them to spurctld, and creates pods.
pub async fn create_spurjob_crd(
    kube_client: &kube::Client,
    namespace: &str,
    session_id: &str,
    name: &str,
    gpu_type: &str,
    gpu_count: i32,
    container_image: &str,
    time_limit_min: i32,
    ssh_enabled: bool,
) -> anyhow::Result<String> {
    let api: Api<SpurJob> = Api::namespaced(kube_client.clone(), namespace);

    let job_name = format!("session-{}", &session_id[..8]);

    let mut labels = BTreeMap::new();
    labels.insert("spur.ai/session-id".to_string(), session_id.to_string());
    labels.insert("spur.ai/managed-by".to_string(), "spur-cloud".to_string());

    let mut env = HashMap::new();
    env.insert("GPUAAS_SESSION_ID".to_string(), session_id.to_string());

    let spurjob = SpurJob::new(
        &job_name,
        SpurJobSpec {
            name: name.to_string(),
            image: container_image.to_string(),
            gpus: GpuSpec {
                count: gpu_count as u32,
                gpu_type: Some(gpu_type.to_string()),
            },
            num_nodes: 1,
            tasks_per_node: 1,
            cpus_per_task: 8,
            time_limit: Some(format!("{}m", time_limit_min)),
            command: vec![],
            args: vec![],
            env,
            host_network: false,
            privileged: false,
        },
    );

    let mut spurjob = spurjob;
    spurjob.metadata.labels = Some(labels);
    spurjob.metadata.namespace = Some(namespace.to_string());

    let created = api.create(&PostParams::default(), &spurjob).await?;
    let crd_name = created.metadata.name.unwrap_or_default();

    info!(
        session_id,
        crd_name = %crd_name,
        namespace,
        "SpurJob CRD created for K8s session"
    );

    Ok(crd_name)
}

/// Delete a SpurJob CRD (on session cancellation).
pub async fn delete_spurjob_crd(
    kube_client: &kube::Client,
    namespace: &str,
    session_id: &str,
) -> anyhow::Result<()> {
    let api: Api<SpurJob> = Api::namespaced(kube_client.clone(), namespace);
    let job_name = format!("session-{}", &session_id[..8]);

    match api.delete(&job_name, &DeleteParams::default()).await {
        Ok(_) => {
            info!(session_id, "SpurJob CRD deleted");
            Ok(())
        }
        Err(kube::Error::Api(e)) if e.code == 404 => Ok(()), // already gone
        Err(e) => Err(e.into()),
    }
}

/// Submit a GPU session as a Spur job. Returns the assigned job ID.
///
/// `ssh_port`: If set, passed as GPUAAS_SSH_PORT (used in bare-metal mode for deterministic sshd port).
/// `bare_metal`: If true, clears container_image so Spur runs the job as a bare process.
pub async fn submit_session(
    client: &mut SlurmControllerClient<Channel>,
    name: &str,
    gpu_type: &str,
    gpu_count: i32,
    container_image: &str,
    partition: Option<&str>,
    ssh_enabled: bool,
    time_limit_min: i32,
    session_id: &str,
    ssh_keys: &str,
    ssh_port: Option<u16>,
    bare_metal: bool,
) -> anyhow::Result<u32> {
    let mut environment = HashMap::new();
    environment.insert("GPUAAS_SESSION_ID".into(), session_id.to_string());
    if ssh_enabled && !ssh_keys.is_empty() {
        environment.insert("GPUAAS_SSH_KEYS".into(), ssh_keys.to_string());
    }
    if let Some(port) = ssh_port {
        environment.insert("GPUAAS_SSH_PORT".into(), port.to_string());
    }

    // Create a profile snippet that enforces GPU isolation.
    // Issue #6: export and readonly the GPU env vars so users can't override them.
    let gpu_profile = concat!(
        "# Spur GPU session profile — enforced isolation\n",
        "if [ -n \"$SPUR_JOB_GPUS\" ]; then\n",
        "  export ROCR_VISIBLE_DEVICES=\"$SPUR_JOB_GPUS\"\n",
        "  export HIP_VISIBLE_DEVICES=\"$SPUR_JOB_GPUS\"\n",
        "  export CUDA_VISIBLE_DEVICES=\"$SPUR_JOB_GPUS\"\n",
        "  export GPU_DEVICE_ORDINAL=\"$SPUR_JOB_GPUS\"\n",
        "  readonly ROCR_VISIBLE_DEVICES HIP_VISIBLE_DEVICES CUDA_VISIBLE_DEVICES GPU_DEVICE_ORDINAL\n",
        "  alias rocm-smi='rocm-smi -d $SPUR_JOB_GPUS'\n",
        "  echo \"GPU session: device(s) $SPUR_JOB_GPUS allocated\"\n",
        "fi\n",
    );

    let script = if ssh_enabled {
        // Entrypoint starts sshd then sleeps.
        // GPUAAS_SSH_PORT is set by the platform for bare-metal mode (deterministic port);
        // defaults to 22 for K8s mode where NodePort handles port mapping.
        format!(
            "#!/bin/bash\n\
            cat > /etc/profile.d/spur-gpu.sh << 'PROFILE'\n\
            {gpu_profile}\
            PROFILE\n\
            mkdir -p /root/.ssh && chmod 700 /root/.ssh\n\
            if [ -n \"$GPUAAS_SSH_KEYS\" ]; then\n\
              echo \"$GPUAAS_SSH_KEYS\" > /root/.ssh/authorized_keys\n\
              chmod 600 /root/.ssh/authorized_keys\n\
            fi\n\
            if command -v sshd >/dev/null 2>&1; then\n\
              mkdir -p /run/sshd\n\
              SSH_PORT=${{GPUAAS_SSH_PORT:-22}}\n\
              ssh-keygen -A 2>/dev/null\n\
              /usr/sbin/sshd -D -p $SSH_PORT &\n\
            fi\n\
            exec sleep infinity\n",
        )
    } else {
        format!(
            "#!/bin/bash\n\
            cat > /etc/profile.d/spur-gpu.sh << 'PROFILE'\n\
            {gpu_profile}\
            PROFILE\n\
            exec sleep infinity\n",
        )
    };

    let spec = JobSpec {
        name: name.to_string(),
        partition: partition.unwrap_or_default().to_string(),
        num_nodes: 1,
        num_tasks: 1,
        cpus_per_task: 8, // proportional: 8 CPUs per GPU
        gres: vec![format!("gpu:{}:{}", gpu_type, gpu_count)],
        script,
        environment,
        time_limit: Some(prost_types::Duration {
            seconds: time_limit_min as i64 * 60,
            nanos: 0,
        }),
        interactive: true,
        // Bare-metal mode: skip container image, run as bare process
        container_image: if bare_metal {
            String::new()
        } else {
            container_image.to_string()
        },
        ..Default::default()
    };

    let resp = client
        .submit_job(SubmitJobRequest { spec: Some(spec) })
        .await?;

    let job_id = resp.into_inner().job_id;
    debug!(job_id, name, "submitted session to spur");
    Ok(job_id)
}

/// Get job info from Spur.
pub async fn get_job(
    client: &mut SlurmControllerClient<Channel>,
    job_id: u32,
) -> anyhow::Result<Option<JobInfo>> {
    match client.get_job(GetJobRequest { job_id }).await {
        Ok(resp) => Ok(Some(resp.into_inner())),
        Err(e) if e.code() == tonic::Code::NotFound => Ok(None),
        Err(e) => Err(e.into()),
    }
}

/// Cancel a Spur job.
pub async fn cancel_job(
    client: &mut SlurmControllerClient<Channel>,
    job_id: u32,
) -> anyhow::Result<()> {
    client
        .cancel_job(CancelJobRequest {
            job_id,
            signal: 0,
            user: String::new(),
        })
        .await?;
    Ok(())
}

/// Get GPU capacity across all nodes.
pub async fn get_gpu_capacity(
    client: &mut SlurmControllerClient<Channel>,
) -> anyhow::Result<Vec<spur_cloud_common::gpu_types::GpuPool>> {
    use spur_cloud_common::gpu_types::{GpuNodeInfo, GpuPool};
    let resp = client.get_nodes(GetNodesRequest::default()).await?;

    let nodes = resp.into_inner().nodes;
    let mut pools: HashMap<String, GpuPool> = HashMap::new();

    for node in &nodes {
        let total_res = node.total_resources.as_ref();
        let alloc_res = node.alloc_resources.as_ref();

        if let Some(total) = total_res {
            for gpu in &total.gpus {
                let pool = pools
                    .entry(gpu.gpu_type.clone())
                    .or_insert_with(|| GpuPool {
                        gpu_type: gpu.gpu_type.clone(),
                        total: 0,
                        available: 0,
                        allocated: 0,
                        memory_mb: gpu.memory_mb,
                        nodes: Vec::new(),
                    });
                pool.total += 1;
            }
        }

        if let Some(alloc) = alloc_res {
            for gpu in &alloc.gpus {
                if let Some(pool) = pools.get_mut(&gpu.gpu_type) {
                    pool.allocated += 1;
                }
            }
        }

        // Build per-node info
        let total_gpus = total_res.map(|r| r.gpus.len() as u32).unwrap_or(0);
        let alloc_gpus = alloc_res.map(|r| r.gpus.len() as u32).unwrap_or(0);

        if total_gpus > 0 {
            let gpu_type = total_res
                .and_then(|r| r.gpus.first())
                .map(|g| g.gpu_type.clone())
                .unwrap_or_default();

            if let Some(pool) = pools.get_mut(&gpu_type) {
                pool.nodes.push(GpuNodeInfo {
                    name: node.name.clone(),
                    total_gpus,
                    available_gpus: total_gpus.saturating_sub(alloc_gpus),
                    state: format!("{:?}", node.state()),
                });
            }
        }
    }

    // Compute available
    for pool in pools.values_mut() {
        pool.available = pool.total.saturating_sub(pool.allocated);
    }

    Ok(pools.into_values().collect())
}
