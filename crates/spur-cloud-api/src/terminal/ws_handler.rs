use axum::extract::ws::{Message, WebSocket};
use futures_util::{SinkExt, StreamExt};
use k8s_openapi::api::core::v1::Pod;
use kube::api::{Api, AttachParams};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tonic::transport::Channel;
use tracing::{debug, error, warn};

use spur_proto::proto::slurm_agent_client::SlurmAgentClient;
use spur_proto::proto::slurm_controller_client::SlurmControllerClient;
use spur_proto::proto::{AttachJobInput, GetJobRequest};

/// Bridge a WebSocket connection to a kubectl exec session in a pod.
///
/// Flow: xterm.js (browser) <-> WebSocket <-> kube exec <-> bash (pod)
pub async fn handle_terminal(
    socket: WebSocket,
    kube_client: kube::Client,
    namespace: String,
    pod_name: String,
) {
    debug!(pod = %pod_name, ns = %namespace, "terminal session starting");

    let pods: Api<Pod> = Api::namespaced(kube_client, &namespace);

    // Start exec session with interactive TTY
    let attach_params = AttachParams {
        stdin: true,
        stdout: true,
        stderr: false, // tty=true merges stderr into stdout; cannot have both true
        tty: true,
        container: None,
        max_stdin_buf_size: Some(1024),
        max_stdout_buf_size: Some(1024),
        max_stderr_buf_size: Some(1024),
    };

    let mut exec = match pods
        .exec(&pod_name, vec!["bash", "-l"], &attach_params)
        .await
    {
        Ok(e) => e,
        Err(e) => {
            error!("kube exec failed: {e}");
            return;
        }
    };

    let mut stdin = match exec.stdin() {
        Some(s) => s,
        None => {
            error!("no stdin from kube exec");
            return;
        }
    };

    let mut stdout = match exec.stdout() {
        Some(s) => s,
        None => {
            error!("no stdout from kube exec");
            return;
        }
    };

    let (mut ws_sink, mut ws_stream) = socket.split();

    // Task 1: WebSocket → pod stdin
    let stdin_handle = tokio::spawn(async move {
        while let Some(msg) = ws_stream.next().await {
            match msg {
                Ok(Message::Text(text)) => {
                    if stdin.write_all(text.as_bytes()).await.is_err() {
                        break;
                    }
                }
                Ok(Message::Binary(data)) => {
                    if stdin.write_all(&data).await.is_err() {
                        break;
                    }
                }
                Ok(Message::Close(_)) => break,
                Err(_) => break,
                _ => {}
            }
        }
    });

    // Task 2: pod stdout → WebSocket (with send timeout to prevent hangs — Issue #10)
    let stdout_handle = tokio::spawn(async move {
        let mut buf = [0u8; 4096];
        loop {
            match stdout.read(&mut buf).await {
                Ok(0) => break,
                Ok(n) => {
                    let data = String::from_utf8_lossy(&buf[..n]).to_string();
                    match tokio::time::timeout(
                        std::time::Duration::from_secs(5),
                        ws_sink.send(Message::Text(data)),
                    )
                    .await
                    {
                        Ok(Ok(_)) => {}
                        _ => break, // Send timeout or error — client likely disconnected
                    }
                }
                Err(_) => break,
            }
        }
    });

    // Wait for either direction to finish
    tokio::select! {
        _ = stdin_handle => {
            debug!(pod = %pod_name, "terminal stdin closed");
        }
        _ = stdout_handle => {
            debug!(pod = %pod_name, "terminal stdout closed");
        }
    }

    warn!(pod = %pod_name, "terminal session ended");
}

/// Bridge a WebSocket connection to a Spur agent's AttachJob gRPC stream.
/// Used in bare-metal mode — connects directly to the spurd agent on the compute node.
///
/// Flow: xterm.js (browser) <-> WebSocket <-> AttachJob gRPC <-> nsenter bash (job)
pub async fn handle_terminal_spur(
    socket: WebSocket,
    mut controller: SlurmControllerClient<Channel>,
    job_id: u32,
    agent_port: u16,
) {
    debug!(job_id, "spur terminal session starting");

    // Look up which node the job is running on
    let job = match controller.get_job(GetJobRequest { job_id }).await {
        Ok(resp) => resp.into_inner(),
        Err(e) => {
            error!(job_id, "failed to get job info: {e}");
            return;
        }
    };

    let nodelist = &job.nodelist;
    if nodelist.is_empty() {
        error!(job_id, "job has no allocated nodes");
        return;
    }

    let first_node = nodelist.split(',').next().unwrap_or(nodelist).trim();
    let agent_addr = format!("http://{}:{}", first_node, agent_port);
    debug!(job_id, agent = %agent_addr, "connecting to agent");

    let mut agent = match SlurmAgentClient::connect(agent_addr.clone()).await {
        Ok(a) => a,
        Err(e) => {
            error!(job_id, "failed to connect to agent at {agent_addr}: {e}");
            return;
        }
    };

    // Set up mpsc channel for AttachJob input stream
    let (tx, rx) = tokio::sync::mpsc::channel::<AttachJobInput>(256);

    // Send initial message with job_id
    if tx
        .send(AttachJobInput {
            job_id,
            data: Vec::new(),
        })
        .await
        .is_err()
    {
        error!(job_id, "failed to send initial attach message");
        return;
    }

    // Start the bidirectional streaming RPC
    let response = match agent
        .attach_job(tokio_stream::wrappers::ReceiverStream::new(rx))
        .await
    {
        Ok(r) => r,
        Err(e) => {
            error!(job_id, "attach_job RPC failed: {e}");
            return;
        }
    };

    let mut out_stream = response.into_inner();
    let (mut ws_sink, mut ws_stream) = socket.split();

    // Task 1: WebSocket → gRPC stdin
    let stdin_handle = tokio::spawn(async move {
        while let Some(msg) = ws_stream.next().await {
            match msg {
                Ok(Message::Text(text)) => {
                    debug!(job_id, bytes = text.len(), "ws→stdin text");
                    if tx
                        .send(AttachJobInput {
                            job_id,
                            data: text.into_bytes(),
                        })
                        .await
                        .is_err()
                    {
                        warn!(job_id, "stdin channel closed");
                        break;
                    }
                }
                Ok(Message::Binary(data)) => {
                    debug!(job_id, bytes = data.len(), "ws→stdin binary");
                    if tx
                        .send(AttachJobInput {
                            job_id,
                            data: data.to_vec(),
                        })
                        .await
                        .is_err()
                    {
                        warn!(job_id, "stdin channel closed");
                        break;
                    }
                }
                Ok(Message::Close(_)) => {
                    debug!(job_id, "ws close received");
                    break;
                }
                Err(e) => {
                    warn!(job_id, "ws error: {e}");
                    break;
                }
                _ => {}
            }
        }
    });

    // Task 2: gRPC stdout → WebSocket (with send timeout — Issue #10)
    let stdout_handle = tokio::spawn(async move {
        loop {
            match out_stream.message().await {
                Ok(Some(chunk)) => {
                    if chunk.eof {
                        break;
                    }
                    if !chunk.data.is_empty() {
                        let text = String::from_utf8_lossy(&chunk.data).to_string();
                        let send_result = tokio::time::timeout(
                            std::time::Duration::from_secs(5),
                            ws_sink.send(Message::Text(text)),
                        )
                        .await;
                        if !matches!(send_result, Ok(Ok(_))) {
                            break;
                        }
                    }
                }
                Ok(None) => break,
                Err(_) => break,
            }
        }
    });

    // Wait for either direction to finish
    tokio::select! {
        _ = stdin_handle => {
            debug!(job_id, "spur terminal stdin closed");
        }
        _ = stdout_handle => {
            debug!(job_id, "spur terminal stdout closed");
        }
    }

    warn!(job_id, "spur terminal session ended");
}
