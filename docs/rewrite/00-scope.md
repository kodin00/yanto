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

## Feature parity checklist

Legend: **Port** = keep the behavior, **Change** = keep it reshaped, **New** = not in legacy, **Drop** = removed.

### Port / Change

- [ ] **Port** — Auth: first-Owner setup, login/logout, CSRF, sessions revocable server-side
- [ ] **Port** — Users: Owner + Members, invites and password resets (24h, single-use), per-Project permissions `deploy`, `runtime`, `config`, `secrets`, `backups`, `hostnames`, plus app views
- [ ] **Port** — `yanto owner-reset` CLI (replaces `npm run owner:reset`)
- [ ] **Port** — Projects: Git- or image-backed CRUD, compose and `.env` preview/edit, Deploy Token reveal/rotate
- [ ] **Change** — Projects declare loopback-published ports; Yanto detects port collisions
- [ ] **Port** — Deployments: Git clone/pull or image pull → `docker compose up`, stored logs, live SSE log stream, stop, restart, rollback with preview, queue
- [ ] **Port** — Webhooks: `POST /deploy?id=` with a bearer Deploy Token; GitHub push webhook
- [ ] **Port** — Containers: list, start/stop/restart, logs + stream, exec terminal (WebSocket)
- [ ] **Port** — System: host usage (CPU/mem/disk), Docker prune with preview, app logs, health
- [ ] **Port** — Deploy Keys (per-Project SSH keys for cloning)
- [ ] **Change** — Backups: local + downloadable only (R2 dropped); Yanto's SQLite added
- [ ] **Change** — Cloudflare: clients, zones, tunnels, DNS, Hostnames, diagnostics — runtime moves to the host `cloudflared`
- [ ] **Change** — FRP: `frps` Host Daemon status, config, tunnel listing (frpc generation dropped)
- [ ] **Port** — Audit log
- [ ] **Change** — Settings: runtime, Cloudflare, SSH, API Tokens (setup wizard moves partly into `yanto install`)
- [ ] **Change** — MCP: same devops tool surface, rebuilt on `rmcp`, authenticated by API Tokens

### New

- [ ] Hosted Registry (`registry:2`): enable/disable, htpasswd users, repo/tag browse, tag delete, GC
- [ ] Registry Credentials for pulling private external images
- [ ] Cron Entries (`/etc/cron.d/yanto`)
- [ ] Authorized Keys
- [ ] Traefik Ingress Provider
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
