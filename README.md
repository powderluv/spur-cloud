# Spur Cloud

GPU as a Service platform built on [Spur](https://github.com/ROCm/spur), the open-source HPC job scheduler. Provides a web interface for launching GPU sessions, with Spur handling scheduling and placement across GPU nodes.

## Architecture

```
                         USERS
                           |
                   [HTTPS / WSS / SSH]
                           |
        +------------------+------------------+
        |                  |                  |
   +----v----+      +------v------+    +------v------+
   |  React  |      |  SSH into   |   |  API / CLI  |
   |  SPA    |      |  session    |   |  clients    |
   +----+----+      |  pod (sshd) |   +------+------+
        |           +------+------+          |
        +------ HTTPS -----+------ HTTPS ----+
                           |
                  +--------v---------+
                  |   spur-cloud-api |
                  |   (Rust/axum)    |
                  +--+-----+-----+--+
                     |     |     |
          +----------+     |     +-----------+
          |                |                 |
   +------v-------+  +----v-----+    +------v--------+
   | PostgreSQL   |  | spurctld |    | K8s API       |
   | (users,      |  | (gRPC)  |    | (kube exec,   |
   |  sessions,   |  +----+----+    |  pod logs)    |
   |  billing)    |       |         +------+---------+
   +--------------+  +----v----+           |
                     | spur-k8s|           |
                     | operator|<----------+
                     +----+----+
                          |
             +------------+------------+
             |            |            |
        +----v---+   +----v---+   +----v---+
        | Node 1 |   | Node 2 |   | Node N |
        | 8xGPU  |   | 8xGPU  |   | 8xGPU  |
        +--------+   +--------+   +--------+
```

### Components

| Component | Description |
|-----------|-------------|
| **spur-cloud-api** | Rust/axum backend. Manages users, sessions, billing. Talks to Spur via gRPC and K8s API for terminal/logs. |
| **Frontend** | React SPA with Tailwind CSS. Dashboard, session launcher, xterm.js web terminal, SSH key management, billing. |
| **Spur** | HPC scheduler. Handles GPU-aware job placement, backfill scheduling, fair-share priority. |
| **spur-k8s operator** | Creates K8s Pods for scheduled jobs with GPU resource requests (`amd.com/gpu`, `nvidia.com/gpu`). |
| **PostgreSQL** | Platform database for users, sessions, SSH keys, and usage records. |

### Session lifecycle

1. User launches a session from the web UI (selects GPU type, count, container image)
2. `spur-cloud-api` creates a DB record and submits a job to Spur via gRPC
3. Spur's backfill scheduler places the job on a node with available GPUs
4. The K8s operator creates a Pod with the requested GPU resources
5. Background sync detects the running state and updates the session
6. If SSH is enabled, a K8s NodePort Service is created and SSH keys injected
7. User accesses the session via web terminal (WebSocket) or SSH

### Fractional GPU access

Spur already supports fractional GPU allocation. Requesting `gpu:mi300x:1` allocates 1 of 8 GPUs on a node, setting `ROCR_VISIBLE_DEVICES` (AMD) or `CUDA_VISIBLE_DEVICES` (NVIDIA) to isolate the device. Up to 8 sessions can share a single 8-GPU node, each with a different GPU.

### HA headnodes

The Spur controller (`spurctld`) supports K8s Lease-based leader election via `--enable-leader-election`. Deploy as a 3-replica StatefulSet for automatic failover. Standby replicas block until the leader fails to renew the Lease (~15s failover).

## Authentication

Three login methods, all producing the same platform JWT:

| Method | Flow |
|--------|------|
| **Local** | Email/password with Argon2 hashing |
| **GitHub** | OAuth2 App. Redirect → code exchange → upsert user by `github_id` |
| **Okta** | OIDC. Discovery → authorize → ID token validation → group-to-admin mapping |

Configure providers in `spur-cloud.toml`:

```toml
[auth]
jwt_secret = "generate-with-openssl-rand-hex-32"

[auth.github]
enabled = true
client_id = "Iv1.abc123"
client_secret = "secret"

[auth.okta]
enabled = true
issuer = "https://mycompany.okta.com/oauth2/default"
client_id = "0oa123"
client_secret = "secret"
admin_groups = ["gpu-admins"]
```

## Building

### Prerequisites

- Rust 1.82+
- Node.js 18+
- PostgreSQL 15+
- Spur controller running (for runtime; not needed to compile)
- `protoc` (protobuf compiler, for spur-proto dependency)

### Backend

```bash
cargo build --release
```

The binary is at `target/release/spur-cloud-api`.

### Frontend

```bash
cd frontend
npm install
npm run build
```

Static assets are in `frontend/dist/`, served by nginx or embedded.

## Configuration

Copy the example config and edit:

```bash
cp spur-cloud.toml.example spur-cloud.toml
```

Key settings:

```toml
public_url = "https://gpu.example.com"   # For OAuth callbacks

[database]
url = "postgresql://user:pass@localhost:5432/spur_cloud"

[spur]
controller_addr = "http://spurctld:6817"  # gRPC address

[server]
listen_addr = "0.0.0.0:8080"
session_namespace = "spur-sessions"       # K8s namespace for GPU pods
```

## Running locally

```bash
# Start PostgreSQL
docker run -d --name pg -e POSTGRES_DB=spur_cloud -e POSTGRES_PASSWORD=dev -p 5432:5432 postgres:16

# Start Spur controller (separate terminal)
spurctld --listen=[::]:6817

# Start the API server
./target/release/spur-cloud-api --config spur-cloud.toml

# Start the frontend dev server (separate terminal)
cd frontend && npm run dev
```

Open http://localhost:5173 to access the UI.

## Deploying to Kubernetes

```bash
# Create namespaces
kubectl apply -f deploy/k8s/namespace.yaml

# Create secrets
kubectl -n spur-cloud create secret generic spur-cloud-secrets \
  --from-literal=db-password=changeme

# Deploy PostgreSQL, API, and frontend
kubectl apply -f deploy/k8s/configmap.yaml
kubectl apply -f deploy/k8s/postgres.yaml
kubectl apply -f deploy/k8s/gpuaas-api.yaml
kubectl apply -f deploy/k8s/gpuaas-frontend.yaml
```

Ensure GPU nodes are labeled for Spur:

```bash
kubectl label node gpu-node-01 spur.ai/managed=true
kubectl label node gpu-node-01 spur.ai/gpu-type=mi300x
```

## API

| Method | Path | Auth | Description |
|--------|------|------|-------------|
| POST | `/api/auth/register` | No | Create local account |
| POST | `/api/auth/login` | No | Login, get JWT |
| GET | `/api/auth/github` | No | GitHub OAuth redirect |
| GET | `/api/auth/okta` | No | Okta OIDC redirect |
| GET | `/api/auth/providers` | No | List enabled auth providers |
| POST | `/api/sessions` | JWT | Launch GPU session |
| GET | `/api/sessions` | JWT | List sessions |
| GET | `/api/sessions/:id` | JWT | Session detail |
| DELETE | `/api/sessions/:id` | JWT | Terminate session |
| WS | `/api/sessions/:id/terminal` | JWT | WebSocket terminal |
| GET | `/api/gpus` | JWT | GPU capacity by type |
| GET | `/api/users/me/ssh-keys` | JWT | List SSH keys |
| POST | `/api/users/me/ssh-keys` | JWT | Add SSH key |
| GET | `/api/billing/usage` | JWT | Usage records |
| GET | `/api/billing/summary` | JWT | Usage summary |
| GET | `/api/admin/update-check` | JWT (admin) | Check for newer release |

## Auto-Update

`spur-cloud-api` queries the [GitHub releases API](https://api.github.com/repos/ROCm/spur-cloud/releases)
on startup to detect newer versions and logs an info message when an update is
available. The service does **not** self-replace — operators update via image
pull (Docker/K8s) or by replacing the binary and restarting systemd.

### Startup log

Look for `update_check` lines in the API log on boot:

```
INFO update_check: spur-cloud-api: up to date (v0.3.0)
INFO update_check: spur-cloud-api: update available v0.3.0 → v0.3.1 — see https://github.com/ROCm/spur-cloud/releases/tag/v0.3.1
```

Results are cached to `cache_dir` for 1 hour to avoid API rate-limit pressure.

### On-demand check (admin)

```bash
curl -H "Authorization: Bearer $ADMIN_JWT" \
  http://localhost:8080/api/admin/update-check
```

```json
{
  "current_version": "0.3.0",
  "latest_version": "v0.3.1",
  "update_available": true,
  "release_url": "https://github.com/ROCm/spur-cloud/releases/tag/v0.3.1"
}
```

### Configuration

Add an `[update]` section to `spur-cloud.toml`:

```toml
[update]
check_on_startup = true     # default: true
channel          = "stable" # "stable" or "nightly"
cache_dir        = "/var/cache/spur-cloud"
```

Set `check_on_startup = false` for air-gapped deployments.

### Updating

| Deployment | How to update |
|------------|---------------|
| Docker / K8s | Bump image tag (e.g. `ghcr.io/rocm/spur-cloud-api:v0.3.1`) and roll the deployment |
| Binary / systemd | Download the release tarball, replace `bin/spur-cloud-api`, `systemctl restart spur-cloud-api` |
| Frontend | Replace `frontend/` static assets with the new release's `frontend/` directory |

## Project structure

```
spur-cloud/
  crates/
    spur-cloud-api/          # Rust backend (axum)
      src/
        auth/                 # JWT, GitHub OAuth, Okta OIDC, CSRF
        db/                   # PostgreSQL repos (users, sessions, billing)
        routes/               # HTTP handlers
        terminal/             # WebSocket <-> kube exec bridge
        ssh/                  # K8s Service lifecycle for SSH access
    spur-cloud-common/        # Shared types
  frontend/                   # React + Vite + Tailwind
    src/
      pages/                  # Login, Dashboard, NewSession, SessionDetail, Settings, Billing
      components/             # Terminal, GpuCapacityCard, SessionTable, Navbar
  deploy/
    docker/                   # Dockerfiles for API, frontend, GPU session image
    k8s/                      # K8s manifests (namespace, RBAC, deployments)
```

## License

Apache-2.0. See [LICENSE](LICENSE).
