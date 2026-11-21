> Classification: INTERNAL

# 06 — Milestones and execution

Every milestone needs a go/no-go from the Owner before it starts (decision Q20). M0 starts right after these docs are merged.

## How work is run

- **Branches:** each milestone gets its own branch `m<N>-<slug>` in its own git worktree, cut from `rewrite/rust`. It merges back with `--no-ff` once the milestone's exit criteria pass.
- **Parallelism:** milestones that share no files run in parallel worktrees with one subagent each. They depend only on the crates M0 and M1 establish.
- **Commits:** small and frequent, one logical change each (a migration, a service op, an adapter, a template, a test). CI must stay green on every merge into `rewrite/rust`.
- **REST and MCP:** every milestone ships the REST endpoints for its features. MCP tools come in M8 because of the `rmcp` risk (ADR 0005).
- **Parity:** a milestone is done when its boxes in the [00-scope parity checklist](00-scope.md#feature-parity-checklist) are ticked.

## Milestones

| M | Name | Contents | Exit criteria | Depends on |
|---|---|---|---|---|
| M0 | Skeleton | Cargo workspace, `mise.toml`, CI (fmt, clippy, test, `sqlx prepare --check`, cargo-audit, gitleaks), config loading, tracing, SQLite pool + embedded migrations, `yanto serve` with `/healthz`, base layout template, vendored htmx served from embedded assets | `cargo build` makes a static musl binary that serves the layout on `127.0.0.1:9197`; CI is green | — |
| M1 | Identity | Setup code + first Owner, login/logout, server-side sessions, CSRF, Members, invites and resets, per-Project permissions and app views, API Tokens, audit log core, `yanto owner-reset` | Permission-matrix tests pass for every role and scope; the audit log records auth events | M0 |
| M2 | Projects & Deployments | Projects CRUD, compose/`.env` edit, loopback ports + collision check, Deploy Keys, Registry Credentials (pull side), deployment queue, `docker compose` runner with streamed logs (SSE), stop/restart/rollback, `/deploy` webhook, GitHub webhook | A Git Project and an image Project each deploy end to end on a test VPS and roll back | M1 |
| M3 | Containers & System | Container list/actions/logs/stream, exec terminal over WebSocket (vendored xterm.js), host usage, Docker prune with preview | The exec session is audited and gated by `runtime` | M1 |
| M4 | Install & Release | `install.sh`, `yanto install` wizard (flags + `--non-interactive`), systemd unit, `yanto self-update`, uninstall, GitHub Actions release (musl amd64/arm64 + SHA256) | A fresh Debian 12 and Ubuntu 22.04 VM install from a release with one command; self-update works | M0 (dogfood after M2) |
| M5 | Host management | Cron Entries (`/etc/cron.d/yanto`, `yanto cron-run` wrapper if adopted), Authorized Keys (marked block, lockout guard), local Backups (volumes, DB dumps, Yanto SQLite) + download + `yanto backup` | Cron and `authorized_keys` writes are atomic and never touch foreign lines (tested against fixture files) | M1 |
| M6 | Hosted Registry | Enable/disable `registry:2`, htpasswd users, repo/tag browser, tag delete, GC, Projects deploying from it | Push → deploy from the Hosted Registry works locally; GC reclaims space | M2 |
| M7 | Ingress & FRP | Ingress Provider trait; Cloudflare clients, zones, tunnels, DNS, diagnostics via the host `cloudflared`; Traefik file provider; `frps` status, config, proxy listing | A Hostname works through each provider on a test VPS; missing daemons show install instructions | M2 |
| M8 | MCP | `rmcp` adapter over the service layer, streamable HTTP + `yanto mcp` stdio, API Token auth, `confirm: true` on sensitive ops | An MCP client can list Projects and trigger a deploy over both transports | M2–M7 |
| M9 | Cutover | Parity checklist complete, README rewritten, security review, remove the TS tree (`src/`, `node_modules` config, Drizzle, Dockerfile, compose files), merge `rewrite/rust` → `master`, tag `v1.0.0` | All parity boxes ticked; the release workflow publishes v1.0.0 | all |

Parallel lanes after M1: **M2 → (M6, M7)**, with **M3**, **M4** and **M5** running alongside M2.

## Top risks

| Risk | Area | Mitigation |
|---|---|---|
| `docker compose` runs as a subprocess (bollard has no Compose API): process lifecycle, cancellation, version drift | Reliability | One runner module with `kill_on_drop`, timeouts, and streamed output; minimum Compose version checked at startup and in `yanto install` |
| Root process editing host files (cron, `authorized_keys`) could lock the operator out or corrupt files | Access control / integrity | Atomic write-and-rename, Yanto-owned file or marked block only, fixture-based tests, refuse to remove the last Authorized Key in the block when it is the session's key (see 04-features) |
| Docker socket = root on the Host; any RBAC bug is a host compromise | Access control | Deny-by-default permission checks in the service layer (not the adapters), permission-matrix tests in M1, security review in M9 |
| `rmcp`, `bollard`, `sqlx` and `argon2` APIs are pre-1.0 and changing | Supply chain / maintenance | Pin exact versions in `Cargo.lock`, cargo-audit in CI, MCP isolated in M8 so it can slip without blocking v1.0.0 |
| An htmx UI for live views (logs, terminal) is new ground for this codebase | Delivery | SSE for logs and WebSocket for exec only; vendored, version-pinned htmx and xterm.js, no npm |
| Cloudflare request-body limits may break large pushes to the Hosted Registry through a tunnel (limit unverified) | Availability | Document FRP or Traefik as the remote push path; test with a large image in M6 |
