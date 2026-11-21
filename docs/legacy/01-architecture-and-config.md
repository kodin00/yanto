# Architecture & Configuration

## Server Bootstrap & Lifecycle (master)

The master process runs an HTTP server. On startup it performs this ordered sequence:

1. **Validate config / warn on unsafe defaults** — checks `YANTO_NODE_ROLE`, the FRP port range, and warns (master only) about default/placeholder secrets (`JWT_SECRET`, admin password, worker token secret, worker join token, MCP token secret, mismatched Turnstile keys, HTTPS base URL without secure cookies).
2. **Run DB migrations** — under a Postgres advisory lock (`pg_advisory_lock(hashtext('yanto.schema.migrations'))`), including a legacy-history compatibility bridge and data backfills.
3. **Load runtime settings** — load runtime-configurable settings from the `app_settings` table (these override env defaults at runtime).
4. **Emit setup code if configured** — if `YANTO_SETUP_CODE` is set, log a one-time setup code.
5. **Ensure local master node** — guarantee the `node_master_local` node row exists (role=master, status=online).
6. **Recover interrupted deployments** — mark deployments left in `running` state by a crash as `failed` (with a "Yanto restarted" notice), since in-flight local runs are gone.
7. **Recover interrupted agent runs** — remove any legacy agent containers labeled `com.yanto.agent=true`, mark interrupted runs `failed` and tasks `done`.
8. **Archive completed agent tasks** — archive `done` tasks older than 3 days.
9. **Start a background archive timer** — `setInterval` (1 hour, unref'd) to periodically archive completed agent tasks.
10. **Listen** on `PORT` (default 8080).
11. **On listen (master)** — warm Codex account status; reconcile cloudflared connectors and tunnel assignments.

### Express middleware pipeline (order matters)

1. **Body parsing** — `express.json` / `urlencoded` with 1 MB limit, plus raw-body capture (for webhook signature verification).
2. **cookieParser** — parses `yanto_session` and `yanto_csrf` cookies.
3. **helmet** — strict CSP: `default-src 'self'`; allows Cloudflare Turnstile challenge scripts/frames from `https://challenges.cloudflare.com`.
4. **CSRF protection** — double-submit cookie (`yanto_csrf`) + `x-csrf-token` header, timing-safe compare, token rotation after each successful mutation. CSRF-exempt paths: `/deploy`, `/webhooks/github`, `/mcp`, and anything under `/api/workers/` (worker token auth instead).
5. **Routers mounted** — `mcp`, `auth`, `users`, `projects`, `deployments`, `containers`, `backups`, `workers`, `settings`, `cloudflare`, `system`, `frp`, `mcp-tokens`, `ai-providers`, `agent-tasks`.
6. **Centralized error handler** — `ZodError` → 400, `HttpError` → its status, else 500. In production, generic 500s are masked as `"Internal server error."` unless they are `HttpError`s.
7. **Static SPA fallback** — serves the built client; a catch-all `GET /*` returns `index.html` for client-side routing.

### Shutdown (SIGINT/SIGTERM)

Idempotent `shutdown()`:
1. Clear the archive timer.
2. Begin closing the HTTP server.
3. In parallel, drain active agent runs and active deployments (default 30s timeout).
4. If either drain times out, force-close all connections.
5. Race `server.close` against the shutdown timeout, then close the DB pool and exit.

## Configuration System

A single frozen `config` object loaded from environment at startup. Integer env vars are parsed strictly (throw on non-integer/out-of-range). Secrets fall back through a chain that rejects known placeholder values.

### Environment variables

| Env var | Default | Notes |
|---|---|---|
| `NODE_ENV` | `development` | |
| `YANTO_NODE_ROLE` | `master` | `master` or `worker` |
| `YANTO_LOCAL_NODE_ID` | `node_master_local` | ID of the local master node row |
| `PORT` | 8080 | 1–65535 |
| `DATABASE_URL` | `postgres://yanto:yanto@localhost:5432/yanto` | |
| `JWT_SECRET` | `change-this-to-a-long-random-secret` (unsafe) | HS256 session signing |
| `WORKER_JOIN_TOKEN` | `""` | One-time enrollment token for workers |
| `WORKER_TOKEN_SECRET` | falls back to `JWT_SECRET` | Secret for worker bearer tokens |
| `YANTO_WORKER_NAME` | `""` | Worker display name (falls back to hostname) |
| `YANTO_WORKER_TOKEN` | `""` | Pre-issued worker bearer token |
| `YANTO_WORKER_TOKEN_PATH` | `/data/worker-token` | Where worker persists its token |
| `YANTO_MASTER_URL` | `""` | Required in worker mode |
| `YANTO_WORKER_POLL_INTERVAL_MS` | 3000 | min 250, max 300000 |
| `YANTO_WORKER_REQUEST_TIMEOUT_MS` | 30000 | 1s..5min |
| `TURNSTILE_SITE_KEY` / `TURNSTILE_SECRET_KEY` | `""` | Cloudflare Turnstile; both must be set together |
| `MCP_TOKEN_SECRET` | falls back to `WORKER_TOKEN_SECRET`/`JWT_SECRET` | MCP token signing |
| `MCP_ALLOWED_HOSTS` / `MCP_ALLOWED_ORIGINS` | `""` | MCP host/origin allowlists (CSV) |
| `YANTO_SETUP_CODE` | `""` | One-time bootstrap setup code |
| `ADMIN_USERNAME` / `ADMIN_PASSWORD` | `""` | Legacy admin bootstrap (migrated into owner on first run if users table empty; password accepted as bcrypt hash if it starts with `$2`) |
| `PROJECTS_ROOT` | `/projects` | |
| `AGENT_WORKTREES_ROOT` | `$PROJECTS_ROOT/.yanto-worktrees` | |
| `HOST_PROJECTS_ROOT` | `~/projects` | |
| `HOST_AGENT_WORKTREES_ROOT` | derived | |
| `BACKUPS_DIR` | `/data/backups` | |
| `CLOUDFLARED_DIR` | `/data/cloudflared` | |
| `FRP_DATA_DIR` | `/data/frp` | |
| `FRP_TOKEN_PATH` | `/data/frp/auth-token` | |
| `FRP_DASHBOARD_URL` | `http://frps:7500` | |
| `FRP_CONTAINER_NAME` | `yanto-frps` | |
| `FRP_BIND_PORT` | 7000 | |
| `FRP_PORT_START` / `FRP_PORT_END` | 25560 / 25600 | Remote port allocation range |
| `SSH_KEYS_DIR` | `/tmp/yanto-ssh` | |
| `SSH_PRIVATE_KEY_PATH` | `/root/.ssh/id_ed25519` | |
| `MANAGED_SSH_PRIVATE_KEY_PATH` | `$SSH_KEYS_DIR/id_ed25519` | |
| `SSH_STRICT_HOST_KEY_CHECKING` | `accept-new` | |
| `APP_BASE_URL` | `http://localhost:$PORT` | |
| `BACKUP_UPLOAD_MAX_BYTES` | 1 GiB | |
| `COMMAND_TIMEOUT_MS` | 1 hour | Generic command timeout |
| `COMMAND_OUTPUT_MAX_BYTES` | 2 MiB | |
| `DEPLOYMENT_LOG_MAX_CHARS` | 500000 | |
| `CODEX_HOME` | `/data/codex` | Codex CLI home |
| `AGENT_MAX_CONCURRENT_RUNS` | 2 | |
| `AGENT_MAX_TURNS` | 40 | |
| `AGENT_RUN_TIMEOUT_MS` | 1 hour | |
| `AGENT_SHUTDOWN_TIMEOUT_MS` | 30000 | Drain timeout for agents AND deployments |
| `AGENT_COMMAND_TIMEOUT_MS` | 10 min | |
| `AGENT_COMMAND_OUTPUT_MAX_BYTES` | 512 KiB | |
| `AGENT_PERSISTED_TOOL_PAYLOAD_MAX_BYTES` | 32 KiB | |
| `COOKIE_SECURE` | auto (true if prod or HTTPS base URL) | |

### Runtime settings

A subset of config is **mutable at runtime** via the `app_settings` table and loaded at startup / on demand:

- `appBaseUrl`
- `cookieSecure`
- `turnstileSiteKey`, `turnstileSecretKey`
- `commandTimeoutMs`
- `commandOutputMaxBytes`
- `deploymentLogMaxChars`

`cookieSecure` from runtime settings drives cookie flags. The Turnstile secret key can be cleared at runtime.

## Deployment Model

Yanto ships as a single multi-arch Docker image (`linux/amd64` + `linux/arm64`), tagged by semantic version. The default stack is three services:

- **`app`** (role `master`) — the Yanto server. Binds `127.0.0.1:$PORT:8080` (intended to stay private / behind a tunnel). Healthcheck `GET /api/health`. Depends on postgres being healthy.
- **`frps`** (same image, separate entrypoint running `frps -c /etc/frp/frps.toml`) — the FRP server. Generates a random auth token on first start, uses it as the dashboard password and shared token. Exposes the control port and the forwarded port range on both tcp and udp. Dashboard on `:7500`.
- **`postgres`** (`postgres:17-alpine`) — database `yanto`, healthcheck `pg_isready`.

**Volumes:** `yanto_postgres`, `yanto_ssh`, `yanto_backups`, `yanto_frp`, `yanto_codex`, `yanto_config`.

**Host bind mounts:** `/var/run/docker.sock` (Docker control), `${HOST_PROJECTS_ROOT:-~/projects}:/projects` (project repos), `${SSH_SOURCE_DIR:-~/.ssh}:/root/.ssh:ro` (read-only host SSH).

Signing secrets are auto-generated and persisted in `/data/config` if not supplied; UI settings are stored in PostgreSQL.

## Install Script (`scripts/install.sh`)

Must be run as root (`umask 077`). Usage: `install.sh master|worker [--master URL] [--join-token TOKEN] [--name NAME] [--dir PATH]`.

1. Parse role and flags; default worker name to hostname, install dir to `/opt/yanto`, repo `https://github.com/kodin00/yanto.git`, branch `master`.
2. If `git` and `docker` are not both present, require apt-based Ubuntu/Debian and `apt-get install` ca-certificates, curl, git, docker.io, docker-compose-plugin; enable+start docker.
3. Clone or update the repo into the install dir.
4. Branch on role:
   - **master**: `docker compose -f compose.yml pull; docker compose -f compose.yml up -d`; prints "open http://127.0.0.1:8080 and create the first owner."
   - **worker**: requires `--master` and `--join-token`. Writes a `chmod 600` `.env.worker` with `YANTO_NODE_ROLE=worker`, `YANTO_MASTER_URL`, `WORKER_JOIN_TOKEN`, `YANTO_WORKER_NAME`, `HOST_PROJECTS_ROOT=/opt/yanto-projects`, `SSH_SOURCE_DIR=/root/.ssh`, command timeouts. Then `docker compose -f compose.worker.yml --env-file .env.worker pull && up -d`.

## Docker Entrypoint (`scripts/docker-entrypoint.sh`)

A minimal shim run before the CMD:

1. Ensure `${YANTO_CONFIG_DIR:-/data/config}` exists (mode 700).
2. **JWT secret bootstrap**: if `JWT_SECRET` is unset, generate (or reuse) a 32-byte hex random secret at `/data/config/jwt-secret` (umask 077) and export it.
3. **Worker token secret bootstrap**: same pattern for `WORKER_TOKEN_SECRET` at `/data/config/worker-token-secret`.
4. `exec "$@"` — hand off to the CMD (master or worker entrypoint).

> Note: database migrations are NOT in the entrypoint. They are run by the server process at startup.

## CLI: `owner-reset`

A standalone entry point for owner account recovery:
1. Run migrations (ensure schema is current).
2. Revoke the owner's sessions (bump `session_version`) and reset the owner's password, generating a one-time reset token.
3. Print to stdout a confirmation and a one-time reset URL valid for **24 hours**.

Purpose: regain access when the owner password is lost; the printed link is the only path back in.

## Worker Process

Runs when `YANTO_NODE_ROLE=worker`. A long-running poller that connects to the master over HTTP and executes deployment jobs locally.

**Token acquisition:**
1. Read stored token from `YANTO_WORKER_TOKEN` env or the token file (mode 0600, dir 0700).
2. If none and no `WORKER_JOIN_TOKEN`, throw.
3. Otherwise POST `/api/workers/register` with `{ joinToken, name, dockerVersion, labels }` to obtain a bearer token, then persist it to the token path.

**Worker identity:** name (env or hostname); dockerVersion (cached 5 min on success, 30s on failure, from `docker version --format {{.Server.Version}}`); labels `{ hostname, platform, arch }`.

**Two parallel loops** (run under one `AbortController`):

- **Control loop** — repeatedly POST `/api/workers/heartbeat` with identity. On error, exponential backoff (`pollInterval × 2^(errors-1)`, capped 60s). On success, reset errors and wait `pollInterval`.
- **Deployment loop** — repeatedly GET `/api/workers/jobs/next` (returns `{ deployment, project } | null`). If a job is returned, run it. Same backoff on errors.

**`runJob`:**
1. Append a "Worker accepted deployment" log to master.
2. Execute the deployment locally (same `runProjectDeployment` logic as the master), streaming log chunks and metadata updates back to the master.
3. On success: PATCH the deployment `success`, exitCode 0.
4. On failure: append a failure log and PATCH `failed`, exitCode 1 (best-effort, errors swallowed).
5. On abort (shutdown): PATCH `failed` with a 5s timeout (best-effort to release the master's running-deployment lock), then return without throwing.

**Shutdown:** SIGINT/SIGTERM abort the controller; loops exit on next poll check. The token is never deleted.

## Logging

A minimal in-process logger with three levels (`info`, `warn`, `error`). Each entry is a JSON line `{ level, message, time (ISO), ...meta }`. It keeps an in-memory ring buffer of the last **500** lines, exposed via `logger.history()`. No external transport, no file output.

## Pagination

`requestPagination(query, defaultPageSize=10, maxPageSize=100)` parses `page` (default 1, min 1) and `pageSize` (clamped 1..maxPageSize). The paired response shape is `PaginatedResponse<T> = { items, total, page, pageSize }`.
