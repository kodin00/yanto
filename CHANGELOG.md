# Changelog

## 1.0.0

First release of the Rust rewrite. Yanto is now a single static binary that runs as a root systemd service on one Linux VPS. There is no import path from the TypeScript 0.x releases (fresh start, ADR 0004).

### Scope

- Projects from a Git repository or a container image, deployed with Docker Compose. Deployments are queued, log to a stored and live-streamed log, and can be stopped, restarted and rolled back (each Deployment snapshots its compose file and encrypted `.env`).
- Containers: list, start/stop/restart, logs and stream, exec terminal (dashboard only, gated by the `runtime` permission and audited).
- Webhooks: `POST /deploy` with a Deploy Token and the GitHub push webhook.
- Users: Owner and Members with per-Project permissions, invites, password resets, server-side sessions, CSRF.
- API Tokens (`read` < `write` < `admin`) for the `/api/v1` REST API and for MCP (HTTP at `/mcp` and stdio via `yanto mcp`).
- Hostnames through a pluggable Ingress Provider: Cloudflare Tunnel (remote-managed, host `cloudflared`) or host Traefik (file provider). Cloudflare clients, zones, tunnels and DNS.
- Host Daemons (`cloudflared`, `frps`, Traefik): checked and driven, never installed. `frps` status, config and live proxies.
- Hosted Registry (`registry:2` on `127.0.0.1:5000`), Registry Credentials, Deploy Keys.
- Host management: Cron Entries (`/etc/cron.d/yanto`, run history through `yanto cron-run`), Authorized Keys (marked block), local downloadable Backups (Project volumes, Postgres/MySQL dumps, Yanto's own SQLite plus `app.key`).
- Settings: runtime values (concurrency, retention, command timeout, log size, trusted proxies) in the dashboard and `/api/v1/settings`.
- Audit log, system usage, protected Docker prune.
- `yanto install` (interactive or `--non-interactive`), `yanto self-update`, `yanto uninstall`, `yanto doctor`, `yanto backup`, `yanto restore`, `yanto owner-reset`, `yanto setup-code`, `install.sh`.
- Release artifacts: static musl binaries for amd64 and arm64, `SHA256SUMS`, build provenance attestations.

### Removed

- AI features: providers and models, Codex account, agent tasks, worktrees, commit/push from the UI.
- Multi-node (Master/Worker, join and worker tokens).
- R2 backup destination (backups are local only).
- Turnstile.
- PostgreSQL and Drizzle (storage is embedded SQLite).
- The React/Vite client (the UI is server-rendered, htmx, embedded in the binary).
- The container and Docker Compose install path and the published container image.
- `frpc` config and script generation, and Yanto-managed `cloudflared` and `frps` containers.
- The separate `ymcp_` MCP token type (folded into API Tokens).

### Security

- The dashboard binds `127.0.0.1:9197` by default; first-Owner setup needs a one-time setup code.
- Secrets at rest are encrypted with an envelope key at `/var/lib/yanto/keys/app.key`.
