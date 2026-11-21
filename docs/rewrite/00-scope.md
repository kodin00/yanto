> Classification: INTERNAL

# 00 — Scope of the Rust rewrite

This doc is the decision record of the planning session (2026-11-21). The terms come from [`CONTEXT.md`](../../CONTEXT.md). The reasoning behind each decision is in [`docs/adr/`](../adr/). The behavior of the legacy system is described in [`docs/legacy/`](../legacy/).

## Product in one paragraph

Yanto v1.0.0 is a single static Rust binary. It runs as a root systemd service on one Linux VPS. It deploys Projects (from Git or an image) with Docker Compose and operates their containers. It manages the Host's cron, Authorized Keys, and Host Daemons (`frps`, `cloudflared`, Traefik), and it can run a Hosted Registry. It has no AI features and never edits application code.

## Fixed decisions

| # | Decision | Source |
|---|---|---|
| D1 | Rewrite in Rust on branch `rewrite/rust` (worktree). TS stays on `master` until the parity checklist below is done, then the Rust tree replaces it. | ADR 0001 |
| D2 | Same repo, Cargo workspace. | — |
| D3 | UI: server-rendered templates + htmx + SSE, embedded in the binary. No Node/npm in the build. | ADR 0003 |
| D4 | Storage: embedded SQLite (WAL) via sqlx. | ADR 0002 |
| D5 | Fresh start. No import from legacy Postgres installs. | ADR 0004 |
| D6 | Native binary only, root systemd service. No container install. Single Host; multi-node is dropped. | ADR 0007 |
| D7 | Toolchain is managed with **mise** (`mise.toml` pins Rust and dev tools). | — |
| D8 | One service layer. Dashboard, `/api/v1` REST, and MCP (HTTP + stdio via `rmcp`) are thin adapters. One API Token type (`read` < `write` < `admin`). If `rmcp` is unstable, drop MCP over HTTP first and keep stdio. | ADR 0005 |
| D9 | Git-sourced deploys stay. | ADR 0001 |
| D10 | Hosted Registry: when enabled, Yanto starts a `registry:2` container on `127.0.0.1:5000` with htpasswd users, a repo/tag browser, tag delete, and GC. Registry Credentials for external registries. | — |
| D11 | Cron = Cron Entries in `/etc/cron.d/yanto` only. Other cron files are shown read-only. | ADR 0007 |
| D12 | SSH = Deploy Keys + Authorized Keys (one configured Unix user, marked block in their `authorized_keys`). | — |
| D13 | Container exec terminal stays, gated by the `runtime` permission and audited. | — |
| D14 | Backups are local only and downloadable: Project volume archives, DB dumps for Projects declaring Postgres/MySQL, and Yanto's SQLite. They can be scheduled through Cron Entries that call `yanto backup …`. | — |
| D15 | Host Daemons (`cloudflared`, `frps`, Traefik) are installed by the operator. Yanto only **checks** for them and drives them; it never installs them. | ADR 0006 |
| D16 | Hostnames route via a pluggable Ingress Provider: Cloudflare Tunnel (remote-managed, host `cloudflared`) or host Traefik (file provider). Cloudflare DNS management is independent of the provider. Targets are Project ports published on `127.0.0.1`. | ADR 0008 |
| D17 | FRP: Yanto manages the `frps` server side only. `frpc` clients and tunnel wiring are manual. | ADR 0006 |
| D18 | Dashboard binds `127.0.0.1:9197` by default (configurable). First-Owner setup is protected by a one-time setup code. | — |
| D19 | `install.sh` is thin: it downloads the latest release binary from GitHub Releases, verifies SHA256, and runs `yanto install`, an interactive wizard where every prompt has a flag (`--non-interactive`). Self-built binaries use the same `yanto install`. `yanto self-update` uses the release path. | — |
| D20 | Targets: Linux + systemd, amd64/arm64, static musl. Tested on Debian 12+ and Ubuntu 22.04+. | — |
| D21 | First Rust release is **v1.0.0**. | — |
| D22 | Secrets at rest: envelope key at `/var/lib/yanto/keys/app.key`. The Yanto-DB Backup bundles the DB and `app.key` into one archive, downloadable by the Owner only and flagged sensitive in the UI. | Owner decision |
| D23 | API Tokens belong to a user and inherit that user's per-Project permissions, capped by token scope (`read` < `write` < `admin`). No per-token Project list. | Owner decision |
| D24 | Container exec is dashboard-only in v1 (no REST or MCP exec). | Owner decision |
| D25 | Each Deployment snapshots its compose file and encrypted `.env`; rollback restores the exact config. | Owner decision |
| D26 | Cron Entries run wrapped as `yanto cron-run <id>`, which records exit code, duration and output tail; the UI shows run history. | Owner decision |
| D27 | The release workflow adds GitHub artifact attestations; `install.sh` verifies with `gh attestation verify` when `gh` is present, else falls back to SHA256. | Owner decision |
| D28 | Applying `frps` config (restarts `frps`, drops live tunnels) requires an explicit confirmation with a warning. | Owner decision |
| D29 | Members may create API Tokens for themselves in the dashboard only, capped by their own permissions. The Owner can list and revoke all tokens. | OQ-17 |
| D30 | Dashboard sessions expire after 24h idle or 7 days absolute, whichever comes first. | OQ-6 |
| D31 | Self-update rollback after migrations is a manual DB restore; the pre-update DB copy is kept under `backups/yanto/`. | OQ-10 |

## Feature parity checklist

Legend: **Port** = keep the behavior, **Change** = keep it reshaped, **New** = not in legacy, **Drop** = removed.

### Port / Change

- [x] **Port** — Auth: first-Owner setup, login/logout, CSRF, sessions revocable server-side
- [x] **Port** — Users: Owner + Members, invites and password resets (24h, single-use), per-Project permissions `deploy`, `runtime`, `config`, `secrets`, `backups`, `hostnames`, plus app views
- [x] **Port** — `yanto owner-reset` CLI (replaces `npm run owner:reset`)
- [x] **Port** — Projects: Git- or image-backed CRUD, compose and `.env` preview/edit, Deploy Token reveal/rotate
- [x] **Change** — Projects declare loopback-published ports; Yanto detects port collisions
- [x] **Port** — Deployments: Git clone/pull or image pull → `docker compose up`, stored logs, live SSE log stream, stop, restart, rollback with preview, queue
- [x] **Port** — Webhooks: `POST /deploy?id=` with a bearer Deploy Token; GitHub push webhook
- [x] **Port** — Containers: list, start/stop/restart, logs + stream, exec terminal (WebSocket)
- [ ] **Port** — System: host usage (CPU/mem/disk), Docker prune with preview, app logs, health
- [x] **Port** — Deploy Keys (per-Project SSH keys for cloning)
- [ ] **Change** — Backups: local + downloadable only (R2 dropped); Yanto's SQLite added
- [x] **Change** — Cloudflare: clients, zones, tunnels, DNS, Hostnames, diagnostics — runtime moves to the host `cloudflared`
- [x] **Change** — FRP: `frps` Host Daemon status, config, tunnel listing (frpc generation dropped)
- [x] **Port** — Audit log
- [ ] **Change** — Settings: runtime, Cloudflare, SSH, API Tokens (setup wizard moves partly into `yanto install`)
- [ ] **Change** — MCP: same devops tool surface, rebuilt on `rmcp`, authenticated by API Tokens

### New

- [x] Hosted Registry (`registry:2`): enable/disable, htpasswd users, repo/tag browse, tag delete, GC
- [x] Registry Credentials for pulling private external images
- [x] Cron Entries (`/etc/cron.d/yanto`)
- [x] Authorized Keys
- [x] Traefik Ingress Provider
- [ ] `/api/v1` REST API with API Tokens
- [ ] `yanto install` interactive wizard and `yanto self-update`

### Drop

- AI providers/models, Codex account, agent tasks/runs/messages/events, agent worktrees, commit/push from UI, `tasks` permission and view, `projects.agent_image`, `yanto_codex` volume
- Multi-node Master/Worker (`compose.worker.yml`, join/worker tokens)
- R2 backup destination
- Turnstile
- `frpc` config/script generation
- Yanto-managed `cloudflared` and `frps` containers
- PostgreSQL, Drizzle, the React/Vite client, and the container/Compose install path
- `ymcp_` token type (folded into API Tokens)

## Roadmap (after v1.0.0, not planned in detail)

- Managing several servers from one dashboard
- systemd service management, firewall, package updates
- Remote backup destinations (S3/R2)

## Resolved during review

Cross-document conflicts that neither the ADRs nor D1–D28 decided, resolved toward the simpler option. Each is applied in docs 01–05.

- Secrets: AES-256-GCM envelope (`enc:v1:…`, AAD = table.column.row id) with the key at `/var/lib/yanto/keys/app.key`; canonical text in 01 §8.3, column inventory in 02 §3.
- Deploy Token: stored as SHA-256 hash (bearer check) plus an encrypted copy (reveal, GitHub HMAC key); see OQ-1.
- Deploy Key private key: encrypted in `deploy_keys.private_key_enc`; materialized to `/var/lib/yanto/ssh/<slug>/id_ed25519` only for the clone; key pairs generated with the `ssh-key` crate, not `ssh-keygen`.
- Cloudflare connector token and `frps` dashboard/auth secrets are not stored in SQLite (fetched from the API / read from `frps.toml`).
- Config split: `config.toml` holds bootstrap and host-integration paths (bind, port, `public_url`, data dir, SSH user, Traefik dir/resolver/entrypoint, `frps` config path, unit names); `app_settings` holds runtime-tunable values only; Hosted Registry enablement is DB state.
- Session cookie is `yanto_session` (no `__Host-` prefix); sessions last 7 days absolute with no sliding window; CSRF token is per session.
- Health: unauthenticated `GET /healthz` (liveness) and `GET /api/v1/health` (`read`, detailed); there is no `/health`.
- Token formats: API Token `yat_<id>_<secret>`, Deploy Token `ydp_<secret>`.
- Deployment `status` includes `interrupted` (startup recovery of `running` rows); `trigger` is `manual|api|deploy_token|github|rollback|cron`; one running and one queued row per Project (partial unique indexes).
- Naming follows 02: `slug` (not `folder_name`), `image_ref`, `compose_path`, `deployment_logs`, one shared `deploy_keys` table, one `registry_credential_id` per Project.
- Deploy limits are `app_settings` keys `deploy.max_concurrent` (2), `runtime.command_timeout_ms`, `runtime.deployment_log_max_bytes` (500 000); drain on shutdown 30 s, `TimeoutStopSec=45`, SIGKILL grace 5 s.
- Webhooks: `/deploy` ignores the request body; rate limits as in 03 §a.4; GitHub body cap 1 MiB; delivery-id dedupe keeps the last 100 ids in memory.
- Hosted Registry: DB is the source of truth for registry users and htpasswd; files live under `/var/lib/yanto/registry/{data,auth}` plus `config.yml`; internal `yanto-internal` password stored encrypted; registry pages are Owner-only.
- Registry auth for pulls uses a throwaway per-command `DOCKER_CONFIG` under `/run/yanto/` (systemd `RuntimeDirectory`); `docker login` is never run and the unit sets no global `DOCKER_CONFIG`.
- Traefik: one Yanto-owned `yanto.yml` in the dynamic dir, targets are plain `http://127.0.0.1:<port>` (no `scheme`/`no_tls_verify`).
- Hostname for the Hosted Registry is a Hostname row with `target_kind='registry'` (Owner-only), needed for remote push; see OQ-9.
- Backups: layout `backups/projects/<slug>/<ts>-<kind>.{tar.zst,sql.zst}` and `backups/yanto/<ts>-yanto.tar.zst`; kinds `project_volumes|project_postgres|project_mysql|yanto_sqlite`; `trigger` adds `pre_restore`; retention is flag-driven (`--keep-last`, `--keep-days`); `backup_download` needs `admin` scope.
- Audit: 02 §4.5 is the single schema (adds `channel`, `outcome`, `request_id`); hash chain, `yanto audit verify|prune` and export are not in v1.0.0; retention default 365 days.
- App logs view reads the in-memory ring buffer (last 500 lines), not `journalctl`.
- Exec routes are `/containers/{id}/exec` (page) and `/exec/ws` (WebSocket) per 03 §b.
- CLI names follow 05 §10: `backup yanto` (not `yanto-db`), `backup project <slug> (--volumes|--db <service>)`, `db status|migrate` (not `migrate --check`), `restore yanto`, `deploy`, `cron-run`; `yanto mcp` reads the token from `YANTO_API_TOKEN_FILE` (or env), never argv; backup lock contention exits 75.
- `yanto install` has no default SSH user (derived from `$SUDO_USER` or required via `--ssh-user`); there is no `root` default.
- The cron file's user field is always `root` and `yanto cron-run` drops to the entry's `run_as_user` (reconciles D26 with the CONTEXT.md Cron Entry wording; see OQ-9).
- Members may create API Tokens for themselves in the dashboard (follows from D23); Owner-only operations stay 403 for them.
- Cloudflare tunnel connector-token reveal is dashboard-only (not REST or MCP), Owner-only and audited.

