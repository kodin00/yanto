# Yanto Deploy

A small deployment platform for a VPS. It registers Git projects, deploys them with Docker Compose, exposes a token-protected deploy endpoint, and gives you a compact dashboard for deployments, containers, logs, and host usage.

## Quick Start

```bash
cp .env.example .env
npm install
npm run build
make docker
```

Open `http://localhost:8080` and create the first owner. Set `YANTO_SETUP_CODE` in `.env`, or read the generated one-time code with `docker compose logs app`.

## One-Line Install

Master node:

```bash
curl -fsSL https://raw.githubusercontent.com/kodin00/yanto/master/scripts/install.sh | sudo bash -s -- master
```

Worker node:

```bash
curl -fsSL https://raw.githubusercontent.com/kodin00/yanto/master/scripts/install.sh | sudo bash -s -- worker --master http://MASTER_IP:PORT --join-token TOKEN
```

The installer supports Ubuntu/Debian hosts. It installs Git and Docker when missing, clones Yanto into `/opt/yanto`, starts Docker Compose, and prints the one-time code used to create the first owner.

## Docker Runtime

The app is designed to run with:

- `/var/run/docker.sock:/var/run/docker.sock` so it can inspect and control Docker.
- `${HOST_PROJECTS_ROOT:-~/projects}:/projects` so registered projects are cloned or deployed from the host projects folder.
- `${SSH_SOURCE_DIR:-~/.ssh}:/root/.ssh:ro` so Git deployments use the SSH key you configured on the VPS.
- A persistent SSH volume at `/data/ssh` is reserved for app-managed keys.

Docker socket access is powerful. Only the Yanto owner can grant delegated project access; keep owner access and the host itself trusted.

## Users and project access

The first account is Yanto's owner. The owner can invite or remove additional username/password accounts from **Settings → Users & Access**, assign projects, and grant project capabilities for deployments, runtime controls, configuration, secrets, backups, and hostnames. Member rows stay compact until selected, while still summarizing their assigned projects and capabilities. Assignment itself grants read access to the project's status, deployment/container logs, and audit entries. Account setup shows the link's username and requires password confirmation.

Nodes, system settings and logs, DNS, FRP, AI Tasks and provider credentials, MCP tokens, host cleanup, and unowned Docker resources remain owner-only. AI Tasks execute host-native commands in the Yanto container and therefore cannot provide a secure project boundary for delegated users. Invite and password-reset links are shown once for the owner to copy and expire after 24 hours.

Project permissions are an application authorization boundary, not hostile-code isolation. A user who can change project code or Compose configuration and deploy it can run that project on the shared Docker host; grant deploy/config access only to trusted project operators.

If the owner is locked out, generate a one-time reset URL from the host:

```bash
docker compose exec app npm run owner:reset
```

Upgrades with explicit `ADMIN_USERNAME` and `ADMIN_PASSWORD` values migrate that account into the database owner automatically. Those variables are ignored after a database owner exists and are not used by fresh installs.

## AI task workspace

The owner-only **AI Tasks** tab adds a Git-backed coding workspace:

- Sign in with a ChatGPT/Codex account using the device-code flow, or register OpenAI Responses, OpenAI-compatible Chat Completions, and Anthropic Messages providers. API keys are encrypted at rest; model IDs can be fetched or entered manually.
- Create a task from a registered Git project, choose the freshly fetched source branch, and create or explicitly resume a task branch.
- Each task gets a real Git worktree under `/projects/.yanto-worktrees` (host-visible at `~/projects/.yanto-worktrees` by default). The project's deployment checkout is never switched by an agent task.
- Codex and API-key provider tools run directly in Yanto's runtime with the task worktree as their working directory; Yanto no longer launches a per-task Docker container or Codex playground. File tools reject traversal, Git metadata, and symlink escapes, and shell tools receive a stripped environment.
- Review streamed tool activity, continue the persistent task conversation, inspect/select changed files, commit, push, and clean the worktree. Retained worktrees have their own manager so completed-task worktrees can be removed manually without deleting task history. Auto-commit, auto-push, and auto-clean are independent per-task switches and default off.

AI tasks require a project with a Git URL and run on the local master node using the bundled Node.js, Python, Git, and ripgrep toolchain. Host-native agent execution is not an OS security boundary, so run tasks only against repositories and instructions you trust.

Codex account sessions are stored per task in the persistent `yanto_codex` volume. Open **AI Tasks → Providers → Sign in with Codex** and follow the verification link. Codex runs through the official SDK with the task worktree as `workingDirectory`, a task-local `CODEX_HOME`, and `danger-full-access` mode so it does not invoke the Linux `bwrap` playground that fails in restricted Yanto containers. Git commit, push, cleanup, and task history remain owned by Yanto.

## Multi-Node Runtime

Yanto has two runtime roles:

- `YANTO_NODE_ROLE=master` runs the dashboard, API, database migrations, and local deploy worker.
- `YANTO_NODE_ROLE=worker` runs only the headless worker process.

Workers poll the master over outbound HTTP. They do not serve the web UI and `compose.worker.yml` does not publish ports.

Set these on the master before connecting workers if you want to provide your own stable secrets:

```env
WORKER_JOIN_TOKEN=<long-random-token>
WORKER_TOKEN_SECRET=<long-random-secret>
APP_BASE_URL=http://MASTER_IP:PORT
```

Worker installs write `.env.worker` with `YANTO_MASTER_URL`, `WORKER_JOIN_TOKEN`, and an optional `YANTO_WORKER_NAME`. After registration, the worker stores its persistent token in the `yanto_worker_data` Docker volume.

If `WORKER_JOIN_TOKEN` is not set, Yanto generates and stores one the first time you copy the worker install command from Settings.

## Deploy Webhook

Each project shows:

```bash
curl -X POST "$APP_BASE_URL/deploy?id=<project-id>" \
  -H "Authorization: Bearer <project-deploy-token>"
```

The webhook and manual deploy button both run the same flow:

1. Clone the repo into `/projects/<folder>` if the folder does not exist.
2. Leave an existing folder untouched.
3. Fetch, checkout, and fast-forward pull the configured branch when the folder is a Git repo.
4. Run `docker compose -f <compose-file> up -d --build`.
5. Store status and build logs.

## Configuration

Important environment variables:

- `DATABASE_URL`
- `JWT_SECRET`
- `MCP_TOKEN_SECRET`, optional; defaults to `WORKER_TOKEN_SECRET`/`JWT_SECRET`; set a stable long random value before creating MCP tokens
- `MCP_ALLOWED_HOSTS`, comma-separated hosts allowed to call `/mcp`
- `MCP_ALLOWED_ORIGINS`, comma-separated browser origins allowed to call `/mcp`
- `YANTO_SETUP_CODE`, required only while claiming a fresh installation; the installer generates it automatically
- `ADMIN_USERNAME` and `ADMIN_PASSWORD`, optional legacy upgrade inputs used once to create the database owner
- `PROJECTS_ROOT` inside the container, default `/projects`
- `HOST_PROJECTS_ROOT` for display, default `~/projects`
- `AGENT_WORKTREES_ROOT` inside the app, default `${PROJECTS_ROOT}/.yanto-worktrees`
- `HOST_AGENT_WORKTREES_ROOT` host-visible worktree path for display, default `${HOST_PROJECTS_ROOT}/.yanto-worktrees`
- `SSH_SOURCE_DIR` host SSH folder mounted read-only into the app, for example `/home/ubuntu/.ssh`
- `SSH_PRIVATE_KEY_PATH` private key path inside the app container, default `/root/.ssh/id_ed25519`
- `SSH_KEYS_DIR`, default `/data/ssh`
- `APP_BASE_URL`
- `YANTO_NODE_ROLE`, default `master`
- `WORKER_JOIN_TOKEN` for worker registration
- `WORKER_TOKEN_SECRET` for worker token signing/hashing configuration
- `YANTO_MASTER_URL` for worker nodes
- `YANTO_WORKER_TOKEN` optional pre-provisioned worker token
- `YANTO_WORKER_NAME` optional display name for worker nodes
- `COMMAND_TIMEOUT_MS`, default `3600000` (one hour) for Git, Docker, and backup helper commands
- `COMMAND_OUTPUT_MAX_BYTES`, default `2097152`, caps in-memory command output while still streaming deployment logs
- `DEPLOYMENT_LOG_MAX_CHARS`, default `500000`, keeps recent deployment logs bounded in Postgres
- `CODEX_HOME`, default `/data/codex`, persistent Codex account and conversation storage
- `AGENT_MAX_CONCURRENT_RUNS`, default `2`
- `AGENT_MAX_TURNS`, default `40` provider/tool iterations per run
- `AGENT_RUN_TIMEOUT_MS`, default `3600000`
- `AGENT_COMMAND_TIMEOUT_MS`, default `600000` for each task shell command
- `AGENT_COMMAND_OUTPUT_MAX_BYTES`, default `524288` per tool result
- `FRP_BIND_PORT`, default `7000`, is the public FRPC control port
- `FRP_PORT_START` and `FRP_PORT_END`, defaults `25560` and `25600`, define the published TCP/UDP forwarding range

## MCP access for AI automation

Yanto exposes the administrator product surface through Model Context Protocol:

- Streamable HTTP: `POST /mcp`
- Local stdio: `npm run start:mcp:stdio` or `node dist/server/server/mcp/stdio.js`

Create tokens from `Settings -> MCP access`. The raw token is shown once; Yanto stores only a keyed hash. Token administration is dashboard/API-only and is not exposed as MCP tools.

Scopes are hierarchical:

- `read`: health, usage, nodes, projects, masked env, logs, deployments, containers, backups, public settings, Cloudflare/FRP reads.
- `write`: includes read plus creates/updates and non-destructive runtime starts.
- `admin`: includes write plus deletion, cleanup, rollback execution, stop/restart operations, and secret/token reveal tools.

Destructive or sensitive MCP calls require both an `admin` token and `confirm: true`. This includes deletion, cleanup, rollback execution, force-delete, deploy token reveal, worker join token reveal, and stop/restart operations. Environment reads are masked; Cloudflare, R2, and SSH secrets are write-only.

HTTP client example:

```json
{
  "mcpServers": {
    "yanto": {
      "url": "http://localhost:8080/mcp",
      "headers": {
        "Authorization": "Bearer ymcp_..."
      }
    }
  }
}
```

Codex stdio example:

```toml
[mcp_servers.yanto]
command = "docker"
args = ["compose", "exec", "-T", "-e", "YANTO_MCP_TOKEN=ymcp_...", "app", "node", "dist/server/server/mcp/stdio.js"]
```

For local development without Docker:

```bash
YANTO_MCP_TOKEN=ymcp_... npm run dev:mcp:stdio
```

MCP intentionally excludes login/logout, webhook ingestion, worker polling/reporting, raw backup download, raw backup upload, and restore flows. Deployment tools return promptly with a deployment ID; poll deployment status or logs through MCP instead of holding the tool call open.

Troubleshooting:

- `401`: missing/invalid bearer token, or the token was revoked.
- `403 MCP host/origin rejected`: add your client host/origin to `MCP_ALLOWED_HOSTS` / `MCP_ALLOWED_ORIGINS`.
- Worker containers do not expose MCP; run MCP against the master app container.

## FRP Port Forwarding

The FRP screen manages TCP and UDP services from a manually run FRPC client, including home servers behind CGNAT. Set the VPS public IP or an unproxied DNS hostname in the FRP screen, create forwarding rules, then copy the client script or `frpc.toml` to the client server.

See [FRP_GUIDE.md](FRP_GUIDE.md) for setup, daily operations, and troubleshooting.

The VPS firewall must allow the FRP control port and configured forwarding range. With the defaults:

```bash
sudo ufw allow 7000/tcp
sudo ufw allow 25560:25600/tcp
sudo ufw allow 25560:25600/udp
```

Yanto does not change firewall rules automatically. A Cloudflare-proxied hostname cannot carry arbitrary Minecraft TCP/UDP traffic; use the VPS IP or a DNS-only hostname.

The generated FRPC config uses `127.0.0.1` for services running on the same client server. Change `localIP`, `localPort`, and `remotePort` in Yanto or in `frpc.toml` for your actual service before starting FRPC.

## Git SSH On A VPS

If Git deploys fail with `Permission denied (publickey)`, test from inside the app container, not only on the host:

```bash
docker compose -f compose.yml exec app ssh -T git@github.com
docker compose -f compose.yml exec app ls -la /root/.ssh
```

Your `.env` should point at the host user's SSH folder:

```env
SSH_SOURCE_DIR=/home/ubuntu/.ssh
SSH_PRIVATE_KEY_PATH=/root/.ssh/id_ed25519
```

Then restart:

```bash
docker compose -f compose.yml up -d --build
```

Alternatively, paste a private key in `Settings -> Git SSH key`. Yanto stores it in `/data/ssh/id_ed25519`, which is persisted by the `yanto_ssh` Docker volume, and uses that key before any mounted VPS key.

## Development

```bash
npm run dev
npm run dev:client
npm run db:generate
npm run db:push
npm run typecheck
npm test
npm run lint
```

## GitHub Actions Deploy

Pushes to `master` run typecheck, lint, tests, build, then deploy to the VPS over SSH.

Add these repository secrets in GitHub under `Settings -> Secrets and variables -> Actions`:

- `VPS_HOST`: VPS hostname or IP address
- `VPS_USER`: SSH user, for example `ubuntu`
- `VPS_SSH_KEY`: private SSH key allowed to log in to the VPS
- `VPS_APP_DIR`: absolute repo path on the VPS, for example `/home/ubuntu/yanto`
- `VPS_PORT`: optional SSH port, defaults to `22`

The SSH user must be able to run Docker with passwordless sudo.

The deploy command is:

```bash
git pull --ff-only origin master
sudo -n docker compose -f compose.yml up -d --build --remove-orphans
```
