use axum::extract::ws::{Message, WebSocket};
use futures_util::{SinkExt, StreamExt};
use k8s_openapi::api::core::v1::Pod;
use kube::api::{Api, AttachParams};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tonic::transport::Channel;
use tracing::{debug, error, info, warn};

use spur_proto::proto::slurm_agent_client::SlurmAgentClient;
use spur_proto::proto::slurm_controller_client::SlurmControllerClient;
use spur_proto::proto::{AttachJobInput, GetJobRequest};

/// Issue #39: WebSocket keepalive interval (30 seconds).
/// Prevents intermediary proxies / firewalls from closing idle connections.
const WS_PING_INTERVAL: std::time::Duration = std::time::Duration::from_secs(30);

/// Issue #39: Maximum time to wait for a WebSocket send before treating it as dead.
const WS_SEND_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(5);

/// Issue #39: Maximum retries for connecting to the agent (bare-metal mode).
const AGENT_CONNECT_RETRIES: u32 = 3;

/// Issue #39: Delay between agent connection retries.
const AGENT_RETRY_DELAY: std::time::Duration = std::time::Duration::from_secs(2);

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

    // Task 1: WebSocket → pod stdin (with ping keepalive — Issue #39)
    let pod_for_log = pod_name.clone();
    let stdin_handle = tokio::spawn(async move {
        let mut ping_interval = tokio::time::interval(WS_PING_INTERVAL);
        ping_interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);

        loop {
            tokio::select! {
                msg = ws_stream.next() => {
                    match msg {
                        Some(Ok(Message::Text(text))) => {
                            if stdin.write_all(text.as_bytes()).await.is_err() {
                                break;
                            }
                        }
                        Some(Ok(Message::Binary(data))) => {
                            if stdin.write_all(&data).await.is_err() {
                                break;
                            }
                        }
                        Some(Ok(Message::Ping(_))) => {
                            // Pong is handled automatically by axum
                            debug!(pod = %pod_for_log, "ws ping received");
                        }
                        Some(Ok(Message::Pong(_))) => {
                            debug!(pod = %pod_for_log, "ws pong received");
                        }
                        Some(Ok(Message::Close(_))) | None => break,
                        Some(Err(_)) => break,
                    }
                }
                // Issue #39: no-op tick — the ping is sent from the stdout task
                // to avoid needing a shared sink. This branch just keeps the
                // select! alive so both directions stay monitored.
                _ = ping_interval.tick() => {}
            }
        }
    });

    // Task 2: pod stdout → WebSocket (with send timeout and ping keepalive — Issue #39)
    let pod_for_log2 = pod_name.clone();
    let stdout_handle = tokio::spawn(async move {
        let mut buf = [0u8; 4096];
        let mut ping_interval = tokio::time::interval(WS_PING_INTERVAL);
        ping_interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);

        loop {
            tokio::select! {
                result = stdout.read(&mut buf) => {
                    match result {
                        Ok(0) => break,
                        Ok(n) => {
                            let data = String::from_utf8_lossy(&buf[..n]).to_string();
                            match tokio::time::timeout(WS_SEND_TIMEOUT, ws_sink.send(Message::Text(data))).await {
                                Ok(Ok(_)) => {}
                                _ => break,
                            }
                        }
                        Err(_) => break,
                    }
                }
                _ = ping_interval.tick() => {
                    // Issue #39: Send WebSocket ping to keep connection alive
                    if ws_sink.send(Message::Ping(vec![].into())).await.is_err() {
                        debug!(pod = %pod_for_log2, "ws ping send failed — client disconnected");
                        break;
                    }
                }
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
///
/// Issue #39: Includes connection retry (up to 3 attempts) and WebSocket ping keepalive.
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

    // Issue #39: Retry agent connection with backoff
    let mut agent = None;
    for attempt in 1..=AGENT_CONNECT_RETRIES {
        match SlurmAgentClient::connect(agent_addr.clone()).await {
            Ok(a) => {
                if attempt > 1 {
                    info!(job_id, attempt, "agent connection succeeded on retry");
                }
                agent = Some(a);
                break;
            }
            Err(e) => {
                warn!(
                    job_id,
                    attempt,
                    max = AGENT_CONNECT_RETRIES,
                    "agent connection failed: {e}"
                );
                if attempt < AGENT_CONNECT_RETRIES {
                    tokio::time::sleep(AGENT_RETRY_DELAY).await;
                }
            }
        }
    }
    let mut agent = match agent {
        Some(a) => a,
        None => {
            error!(
                job_id,
                "failed to connect to agent at {agent_addr} after {AGENT_CONNECT_RETRIES} attempts"
            );
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

    // Task 1: WebSocket → gRPC stdin (with ping keepalive — Issue #39)
    let stdin_handle = tokio::spawn(async move {
        let mut ping_interval = tokio::time::interval(WS_PING_INTERVAL);
        ping_interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);

        loop {
            tokio::select! {
                msg = ws_stream.next() => {
                    match msg {
                        Some(Ok(Message::Text(text))) => {
                            debug!(job_id, bytes = text.len(), "ws→stdin text");
                            if tx.send(AttachJobInput { job_id, data: text.into_bytes() }).await.is_err() {
                                warn!(job_id, "stdin channel closed");
                                break;
                            }
                        }
                        Some(Ok(Message::Binary(data))) => {
                            debug!(job_id, bytes = data.len(), "ws→stdin binary");
                            if tx.send(AttachJobInput { job_id, data: data.to_vec() }).await.is_err() {
                                warn!(job_id, "stdin channel closed");
                                break;
                            }
                        }
                        Some(Ok(Message::Ping(_))) | Some(Ok(Message::Pong(_))) => {
                            // Handled automatically by axum
                        }
                        Some(Ok(Message::Close(_))) | None => {
                            debug!(job_id, "ws close received");
                            break;
                        }
                        Some(Err(e)) => {
                            warn!(job_id, "ws error: {e}");
                            break;
                        }
                    }
                }
                _ = ping_interval.tick() => {}
            }
        }
    });

    // Task 2: gRPC stdout → WebSocket (with send timeout and ping keepalive — Issue #39)
    let stdout_handle = tokio::spawn(async move {
        let mut ping_interval = tokio::time::interval(WS_PING_INTERVAL);
        ping_interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);

        loop {
            tokio::select! {
                result = out_stream.message() => {
                    match result {
                        Ok(Some(chunk)) => {
                            if chunk.eof {
                                break;
                            }
                            if !chunk.data.is_empty() {
                                let text = String::from_utf8_lossy(&chunk.data).to_string();
                                let send_result = tokio::time::timeout(
                                    WS_SEND_TIMEOUT,
                                    ws_sink.send(Message::Text(text)),
                                ).await;
                                if !matches!(send_result, Ok(Ok(_))) {
                                    break;
                                }
                            }
                        }
                        Ok(None) => break,
                        Err(_) => break,
                    }
                }
                _ = ping_interval.tick() => {
                    // Issue #39: Send WebSocket ping to keep connection alive
                    if ws_sink.send(Message::Ping(vec![].into())).await.is_err() {
                        debug!(job_id, "ws ping send failed — client disconnected");
                        break;
                    }
                }
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keepalive_constants_are_reasonable() {
        // Verify keepalive interval is between 10s and 60s
        assert!(WS_PING_INTERVAL.as_secs() >= 10);
        assert!(WS_PING_INTERVAL.as_secs() <= 60);

        // Verify send timeout is shorter than keepalive interval
        assert!(WS_SEND_TIMEOUT < WS_PING_INTERVAL);

        // Verify retry count is reasonable
        assert!(AGENT_CONNECT_RETRIES >= 1);
        assert!(AGENT_CONNECT_RETRIES <= 10);
    }

    #[test]
    fn retry_delay_is_reasonable() {
        // Total retry time should be under 30s
        let total_retry_time = AGENT_RETRY_DELAY.as_secs() * (AGENT_CONNECT_RETRIES as u64 - 1);
        assert!(total_retry_time <= 30);
    }
}
