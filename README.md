# Yanto Deploy

Yanto is a small VPS deployment platform for Git projects and Docker images. It includes a dashboard for deployments, containers, logs, host usage, FRP forwarding, and AI coding tasks.

## Quick Start

Build from source:

```bash
git clone https://github.com/kodin00/yanto.git
cd yanto
docker compose up -d --build
```

Or use the prebuilt image without cloning the repository:

```bash
mkdir yanto && cd yanto
curl -fsSLO https://raw.githubusercontent.com/kodin00/yanto/master/compose.yml
docker compose up -d
```

No `.env` file is required. Both options start Yanto, PostgreSQL, and FRP and persist their data in Docker volumes. Open `http://localhost:8080` and create the first owner. Keep port 8080 private until the owner is created.

The downloaded Compose file pulls `ghcr.io/kodin00/yanto:latest`. Pin a release with `YANTO_VERSION=0.14.0 docker compose up -d`.

## Runtime and security

Yanto needs access to:

- Docker through `/var/run/docker.sock`
- projects at `/projects` (host default: `~/projects`)
- an optional read-only host SSH directory at `/root/.ssh`

Docker access and deployed code are not security boundaries. Keep the host and owner account trusted, and grant deployment or configuration permissions only to trusted operators.

The first account is the owner. Manage other users and project permissions in **Settings → Users & Access**. Invitations and password-reset links expire after 24 hours. If the owner is locked out, run:

```bash
docker compose exec app npm run owner:reset
```

## AI Tasks

The owner-only **AI Tasks** workspace can run Codex, OpenAI-compatible, or Anthropic-backed coding tasks against Git projects. Each task uses a separate worktree under `/projects/.yanto-worktrees`, leaving the deployment checkout untouched.

Tasks can stream tool activity, continue conversations, review changes, commit, push, and clean up worktrees. Auto-commit, auto-push, and auto-clean are off by default. Tasks run inside the Yanto container with host-level capabilities, so use only trusted repositories and instructions.

## Deploy webhook

Each project provides a token-protected deploy endpoint:

```bash
curl -X POST "$APP_BASE_URL/deploy?id=<project-id>" \
  -H "Authorization: Bearer <project-deploy-token>"
```

Deployments clone or update the configured Git branch, or pull the configured image, then run Docker Compose and store the result and logs.

## Configuration

Use the setup wizard or **Settings → Runtime** for the public URL, secure cookies, Turnstile, command limits, and log retention.

Compose provides defaults for optional environment variables. The most commonly changed are:

- `YANTO_VERSION` and `PORT`
- `POSTGRES_PASSWORD` and `DATABASE_URL`
- `HOST_PROJECTS_ROOT` and `SSH_SOURCE_DIR`
- `APP_BASE_URL` and `YANTO_SETUP_CODE`
- `MCP_TOKEN_SECRET`, `MCP_ALLOWED_HOSTS`, and `MCP_ALLOWED_ORIGINS`
- `FRP_BIND_PORT`, `FRP_PORT_START`, and `FRP_PORT_END`

Signing secrets are generated and persisted automatically unless explicitly provided. Settings supplied through the UI are stored in PostgreSQL.

## MCP

Create a scoped token in **Settings → MCP access**. HTTP clients connect to `POST /mcp` with `Authorization: Bearer ymcp_...`. Tokens support `read`, `write`, and `admin` scopes; sensitive admin operations also require `confirm: true`.

For local stdio access:

```bash
docker compose exec -T -e YANTO_MCP_TOKEN=ymcp_... app \
  node dist/server/server/mcp/stdio.js
```

## FRP forwarding

Use the **FRP** screen to expose TCP or UDP services behind CGNAT. Configure the VPS IP or a DNS-only hostname, then copy the generated client script or `frpc.toml` to the client server.

With the default ports, allow:

```bash
sudo ufw allow 7000/tcp
sudo ufw allow 25560:25600/tcp
sudo ufw allow 25560:25600/udp
```

Yanto does not manage firewall rules. See [FRP_GUIDE.md](FRP_GUIDE.md) for setup and troubleshooting.

## Git SSH

If Git authentication fails, verify the key inside the app container:

```bash
docker compose exec app ssh -T git@github.com
docker compose exec app ls -la /root/.ssh
```

Set `SSH_SOURCE_DIR` to the host user’s SSH directory, or add a key in **Settings → Git SSH key**.

## Development

```bash
npm install
npm run dev:full
npm run typecheck
npm test
npm run lint
```

## Releases and deployment

Pushing a semantic version tag such as `v0.14.0` publishes `linux/amd64` and `linux/arm64` images to `ghcr.io/kodin00/yanto`. Stable releases also update `latest`.

Pushes to `master` run typecheck, lint, tests, and build before deploying over SSH. Configure `VPS_HOST`, `VPS_USER`, `VPS_SSH_KEY`, and `VPS_APP_DIR` as GitHub Actions secrets; `VPS_PORT` is optional.
