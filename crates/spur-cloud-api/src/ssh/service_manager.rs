use k8s_openapi::api::core::v1::{Service, ServicePort, ServiceSpec};
use k8s_openapi::apimachinery::pkg::util::intstr::IntOrString;
use kube::api::{Api, DeleteParams, ObjectMeta, PostParams};
use kube::Client;
use std::collections::BTreeMap;
use tracing::{debug, error, info};
use uuid::Uuid;

/// Compute a deterministic SSH port for a session based on its UUID.
/// Used in bare-metal mode where there is no K8s Service abstraction.
pub fn ssh_port_for_session(session_id: &Uuid, base: u16, range: u16) -> u16 {
    let hash = (session_id.as_u128() % range as u128) as u16;
    base + hash
}

/// Create a NodePort Service to expose SSH (port 22) for a session pod.
/// Returns (host, port) for the SSH endpoint.
pub async fn create_ssh_service(
    client: &Client,
    namespace: &str,
    session_id: &str,
    pod_name: &str,
) -> anyhow::Result<(String, i32)> {
    let services: Api<Service> = Api::namespaced(client.clone(), namespace);
    let service_name = format!("ssh-{}", &session_id[..8]);

    let mut selector = BTreeMap::new();
    // Match the pod by its spur job label
    selector.insert("spur-job".to_string(), pod_name.to_string());

    let svc = Service {
        metadata: ObjectMeta {
            name: Some(service_name.clone()),
            namespace: Some(namespace.to_string()),
            labels: Some({
                let mut l = BTreeMap::new();
                l.insert("app".to_string(), "spur-cloud-ssh".to_string());
                l.insert("session-id".to_string(), session_id.to_string());
                l
            }),
            ..Default::default()
        },
        spec: Some(ServiceSpec {
            type_: Some("NodePort".to_string()),
            selector: Some(selector),
            ports: Some(vec![ServicePort {
                name: Some("ssh".to_string()),
                port: 22,
                target_port: Some(IntOrString::Int(22)),
                protocol: Some("TCP".to_string()),
                ..Default::default()
            }]),
            ..Default::default()
        }),
        ..Default::default()
    };

    let created = services.create(&PostParams::default(), &svc).await?;

    // Extract the assigned NodePort
    let node_port = created
        .spec
        .as_ref()
        .and_then(|s| s.ports.as_ref())
        .and_then(|ports| ports.first())
        .and_then(|p| p.node_port)
        .unwrap_or(0);

    info!(service = %service_name, node_port, "SSH service created");

    // The host is the node where the pod is running — caller should look up pod.status.hostIP
    // For now, return empty host (caller fills in from pod status)
    Ok((String::new(), node_port))
}

/// Delete the SSH service for a session.
pub async fn delete_ssh_service(
    client: &Client,
    namespace: &str,
    session_id: &str,
) -> anyhow::Result<()> {
    let services: Api<Service> = Api::namespaced(client.clone(), namespace);
    let service_name = format!("ssh-{}", &session_id[..8]);

    match services.delete(&service_name, &DeleteParams::default()).await {
        Ok(_) => {
            debug!(service = %service_name, "SSH service deleted");
            Ok(())
        }
        Err(kube::Error::Api(e)) if e.code == 404 => {
            debug!(service = %service_name, "SSH service already gone");
            Ok(())
        }
        Err(e) => {
            error!(service = %service_name, "SSH service deletion failed: {e}");
            Err(e.into())
        }
    }
}
