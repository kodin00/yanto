> Classification: INTERNAL

# 01 — Architecture of the Rust rewrite

Binding inputs: [`CONTEXT.md`](../../CONTEXT.md), [`docs/adr/`](../adr/), [`00-scope.md`](00-scope.md) (D1–D21). Nothing here re-decides them. Install, CLI, CI, and release live in [`05-install-and-ops.md`](05-install-and-ops.md).

Conventions: **ASSUMPTION** = not grounded in the provided docs/legacy code/library docs; needs confirmation. Library versions are written only where Context7 docs listed one; otherwise **unverified** (pin at first `cargo add`, verify on crates.io).

## 1. Shape in one picture

```
            ┌────────────── yanto (bin: yanto-cli) ──────────────┐
            │ clap · serve · install · self-update · backup · mcp │
            └──────┬───────────────────────────────┬──────────────┘
                   │ serve                         │ other subcommands
        ┌──────────▼──────────┐                    │
        │ yanto-server        │  adapters only     │
        │ dashboard │ /api/v1 │                    │
        │ MCP (HTTP)│ SSE/WS  │                    │
        └──────────┬──────────┘                    │
                   │ Actor + DTOs                  │
        ┌──────────▼───────────────────────────────▼──┐
        │ yanto-core: domain · services · repo (sqlx) │
        │ queue worker · retention · log hub · ports  │
        └──────────┬──────────────────────────────────┘
                   │ implements ports (traits)
        ┌──────────▼──────────┐
        │ yanto-host          │ bollard · docker compose · git ·
        │ side-effect adapters│ cron.d · authorized_keys · systemd ·
        └─────────────────────┘ Traefik · frps · Cloudflare API · registry
```

## 2. Cargo workspace

Single repo, virtual workspace at the root (D2). Resolver 2, edition 2021 or later (**ASSUMPTION**: edition 2024 if the pinned toolchain allows; decide when `mise.toml` pins Rust).

| Crate | Kind | Responsibility | May depend on |
|---|---|---|---|
| `yanto-core` | lib | Domain types (Project, Deployment, Hostname, …), **the service layer** (one fn per operation), authorization (`Actor`, permissions, token scopes), SQLite repositories + migrations, deployment queue worker, retention tasks, log/event hub, **port traits** (`DockerPort`, `ComposePort`, `GitPort`, `CronPort`, `AuthorizedKeysPort`, `HostDaemonPort`, `IngressProvider`, `RegistryPort`, `CloudflarePort`), config types, `CoreError`. No HTTP framework, no `bollard`, no filesystem writes outside the data dir. | sqlx, tokio, serde, tracing |
| `yanto-host` | lib | Real implementations of the ports: Docker (bollard), `docker compose` runner, `git` runner, cron writer, authorized_keys writer, systemd/daemon checks, Traefik file provider, frps config + dashboard client, Cloudflare API client, Hosted Registry client. One module per concern (§7). | `yanto-core` (ports only), bollard, reqwest, tokio |
| `yanto-server` | lib | The three adapters: htmx dashboard (askama templates, embedded assets), `/api/v1` REST, MCP over streamable HTTP; session/CSRF/token middleware; SSE and WebSocket (exec terminal) endpoints; webhook endpoints. Thin: parse → build `Actor` → call a service fn → render. | `yanto-core`, axum, tower-sessions, askama, rmcp |
| `yanto-cli` | bin `yanto` | clap CLI, composition root (wires `yanto-host` into `yanto-core` services), `serve`, `install` wizard, `self-update`, `owner-reset`, `backup`, `mcp` (stdio adapter, uses `rmcp` stdio + the same tool set exported by `yanto-server::mcp`), `version`. | all three, clap, inquire |

Why this split (short):
- **core vs host**: the service layer must be testable with fakes and never touch `/etc` or Docker in unit tests. Ports in core, impls in host, composition in the CLI. This is the seam ADR 0005 needs (one service layer, many callers).
- **server separate from core**: the CLI `backup`/`owner-reset` paths and stdio MCP must run without pulling axum/askama; compile times of core stay low; adapters cannot leak HTTP types into services.
- **MCP tool definitions live in `yanto-server::mcp`** (module, not crate) and are exposed as a `ToolSet` that both the HTTP service and `yanto mcp` stdio mount. This keeps one definition for both transports. ASSUMPTION: depending on `yanto-server` from the CLI is acceptable; if binary size or compile time matters, extract `yanto-mcp` later.
- Not split further (no per-feature crates): single operator-scale codebase, one release artifact; more crates add friction without a boundary that matters.

Workspace-level: `[workspace.dependencies]` pins every external crate once; `[workspace.lints]` sets `clippy::all` + `unsafe_code = "forbid"` (ASSUMPTION: `nix`/`libc` calls for chown/process groups go through safe wrappers; if raw `unsafe` is needed it is confined to `yanto-host` with `allow` and a comment).

## 3. Module map

```
crates/yanto-core/src/
  lib.rs
  config.rs            // Config structs (parsed from /etc/yanto/config.toml)
  error.rs             // CoreError, ErrorKind
  actor.rs             // Actor { kind, user_id, scope, permissions }, authorize()
  secrets.rs           // Secret<T> (redacting Debug), envelope encrypt/decrypt
  db/                  // pool setup, migrations runner, repo modules
    mod.rs  repo/{users,sessions,projects,deployments,hostnames,cron,keys,tokens,audit,settings,registry,backups}.rs
  service/             // THE service layer; one file per area
    auth.rs users.rs projects.rs deployments.rs containers.rs system.rs
    hostnames.rs cloudflare.rs frp.rs registry.rs cron.rs ssh.rs backups.rs
    tokens.rs settings.rs audit.rs
  jobs/                // background tasks (spawned by `serve` only)
    queue.rs           // deployment queue worker
    retention.rs       // log/audit/session pruning
    hub.rs             // EventHub: tokio broadcast fan-out
  ports/               // trait definitions only (see §7)
  testing/             // fakes (feature "testing")
crates/yanto-host/src/
  docker.rs compose.rs git.rs cron.rs authorized_keys.rs systemd.rs
  ingress/{mod.rs,cloudflare_tunnel.rs,traefik.rs}
  frps.rs cloudflare_api.rs registry.rs hostfs.rs (atomic write, locks, perms)
crates/yanto-server/src/
  lib.rs app.rs        // Router assembly, AppState
  auth/{session_store.rs,csrf.rs,extract.rs,tokens.rs}
  dashboard/           // handlers + templates/ (askama) + assets/ (htmx, css, xterm)
  api_v1/              // REST handlers, DTO <-> JSON, error mapping
  mcp/                 // rmcp tool set + HTTP service builder
  sse.rs ws.rs webhooks.rs assets.rs
crates/yanto-cli/src/
  main.rs cli.rs       // clap definitions
  cmd/{serve,install,self_update,owner_reset,backup,mcp,version,...}.rs
  wizard.rs            // inquire prompts
  systemd_unit.rs      // unit file rendering
```

## 4. One service layer, three adapters (ADR 0005)

**Rule**: all behavior is a method on a service in `yanto-core::service`. Adapters never touch repositories, ports, or the filesystem.

```rust
// shape, illustrative
pub struct Services { pub projects: ProjectService, pub deployments: DeploymentService, /* … */ }

impl DeploymentService {
    pub async fn trigger(&self, actor: &Actor, project: ProjectId, req: TriggerDeployment)
        -> Result<Deployment, CoreError>;
}
```

- **Authorization is in the service**, not the adapter: each fn calls `actor.require(Permission::Deploy, Some(project))` (Owner: always; Member: per-Project grants `deploy|runtime|config|secrets|backups|hostnames` + app views; API Token: scope `read` < `write` < `admin` mapped onto operations, further capped by the token owner's permissions — ASSUMPTION: a token acts as its creating user, scope can only narrow). Deploy Token is a separate `ActorKind::DeployToken(ProjectId)` that may only call `deployments.trigger` for that Project.
- **Audit** rows are written by the service in the same SQLite transaction as the mutation; `Actor` records the channel (`dashboard|rest|mcp|deploy-token|cli`).
- **Operation parity**: each operation has one DTO in/out. REST route, dashboard handler, and MCP tool each map 1:1 to a service fn. Parity is enforced by review plus a test that lists the operation names each adapter exposes against a shared `OPERATIONS` table (ASSUMPTION; cheap to add later).
- **Adapters**:

| Adapter | Transport | Auth | Output |
|---|---|---|---|
| Dashboard | HTML + htmx, SSE for live logs/status, WebSocket for exec terminal | session cookie + CSRF | askama templates; full page or partial depending on `HX-Request` |
| REST `/api/v1` | JSON over HTTP | `Authorization: Bearer <API Token>` | JSON DTOs, uniform error body, pagination `page/pageSize` (as legacy) |
| MCP (HTTP) | `rmcp` streamable HTTP mounted at `/mcp` | API Token | tools backed by service fns |
| MCP (stdio) | `yanto mcp` | API Token from env/flag (§4.1) | same tools |
| Webhooks | `POST /deploy?id=` , `POST /webhooks/github` | Deploy Token / GitHub signature | 202 + Deployment id |

### 4.1 MCP details (grounded in rmcp docs)

- Tools are defined with `#[tool_router]` / `#[tool]` / `#[tool_handler]` (rmcp docs). Parameters use `Parameters<T>` with `schemars::JsonSchema` DTOs.
- HTTP: `StreamableHttpService::new(service_factory, Arc<LocalSessionManager>, StreamableHttpServerConfig)` is a tower service; mount it with axum `nest_service("/mcp", …)` behind the API Token middleware. The middleware's resolved `Actor` is inserted as an http extension; tool handlers read it from `http::request::Parts` via `Extension<Parts>` / `RequestContext` (documented rmcp pattern). Handlers must treat a missing `Actor` as unauthorized.
- Stdio: `handler.serve(rmcp::transport::stdio()).await` then `.waiting()` (rmcp `ServiceExt::serve` docs).
- `yanto mcp` **runs the services in-process against the SQLite DB** (no running server required). It authenticates once at startup with an API Token (`YANTO_API_TOKEN` env, never a CLI flag value to avoid `ps` exposure — ASSUMPTION). Mutations that enqueue a Deployment write to the queue table; the running `serve` process picks them up (§6.1). If `serve` is down, Deployments wait queued.
- Fallback per ADR 0005: if `rmcp` is unstable, remove the HTTP mount; the stdio path and the tool set stay.
- rmcp version: **unverified** (docs show only `latest`). Pin exactly and wrap in `yanto-server::mcp` so API churn stays in one module.

## 5. Request and auth flow

### 5.1 Router layering (`axum`, `tower`)

Outermost → innermost:
1. `TraceLayer` + request-id (tower-http, **unverified** version) — span per request.
2. Security headers: CSP `default-src 'self'` (no third-party origins; htmx/xterm vendored — Turnstile allowance from legacy is dropped), `X-Content-Type-Options`, `Referrer-Policy`, `frame-ancestors 'none'`.
3. Body limit (legacy: 1 MiB default; larger limit only on backup upload route).
4. Per-surface sub-routers:
   - `/` dashboard: session layer → CSRF layer → `AuthenticatedUser` extractor.
   - `/api/v1/*`: bearer-token extractor only; no cookies read, no CSRF (token is not ambient credential).
   - `/mcp`: bearer-token extractor.
   - `/deploy`, `/webhooks/github`: Deploy Token / signature; no session; no CSRF.
   - `/healthz`: unauthenticated, minimal (`ok`), no version info.

### 5.2 Dashboard sessions

- **tower-sessions** with a **custom `SessionStore` over sqlx/SQLite**. Docs show the trait: `create/save/load/delete` (async-trait, `Debug + Send + Sync + 'static`), with `Expiry::{OnSessionEnd, OnInactivity, AtDateTime}` and `SessionManagerLayer::with_expiry`. A custom store (not a stock one) because the parity item "sessions revocable server-side" needs `user_id` as an indexed column (revoke all sessions of a user on password reset / owner-reset / member removal). Stock store crate for sqlx/SQLite was not checked: **unverified**.
- Table `sessions(id, user_id NULL, data BLOB/JSON, expires_at, created_at, last_seen_at, ip, user_agent)`; the store writes `user_id` from session data on `save`. ASSUMPTION: the store parses the `Record` data to extract `user_id`; alternative is a separate `user_sessions` link table written by the login service.
- Expiry: `OnInactivity` (default 7 days — ASSUMPTION) with absolute cap (30 days — ASSUMPTION), enforced in `load`. Note from docs: reads are not activity; only modification extends expiry, so the middleware must `save` periodically (touch `last_seen`).
- Cookie: `HttpOnly`, `SameSite=Lax`, `Secure` when `server.public_url` is `https://` (legacy `cookieSecure` behavior), cookie name `yanto_session`. Session id rotated on login (fixation) and on privilege change.
- Passwords: **argon2id** (crate **unverified**), per-user salt; legacy scrypt/bcrypt shapes are not carried over (ADR 0004). Login rate limiting: in-memory per-IP+username limiter with backoff (ASSUMPTION; no crate chosen). Constant-time compare via `subtle` (**unverified**).
- First Owner: `POST /setup` is enabled only while no Owner exists and requires the one-time **setup code** (hash stored in `settings`, consumed atomically with Owner creation, written by `yanto install`; re-issuable with `yanto setup-code` while no Owner exists; see doc 05).

### 5.3 CSRF (dashboard only)

Synchronizer token stored in the server-side session (not a double-submit cookie; legacy used double-submit, but we now have server state, so it is simpler and stronger). Rules:
- Every unsafe method (`POST/PUT/PATCH/DELETE`) on dashboard routes requires `X-CSRF-Token` header **or** `_csrf` form field; htmx sends it via `hx-headers` on `<body>` (`{"X-CSRF-Token": "{{ csrf }}"}`), plain forms include the hidden field.
- Additionally validate `Origin` (or `Referer` fallback) against `public_url`/request host; WebSocket upgrade (exec terminal) validates `Origin` and requires the CSRF token as a query/first message (ASSUMPTION).
- Token rotates on login only (not per-mutation as legacy did; per-mutation rotation breaks htmx multi-request pages).
- Exempt: `/api/v1`, `/mcp`, `/deploy`, `/webhooks/github`.

### 5.4 Tokens

| Credential | Format | Storage | Verified by | Revealable? |
|---|---|---|---|---|
| Session | tower-sessions id | `sessions` table | cookie lookup | n/a |
| API Token | `yanto_` + 32 random bytes base64url (prefix ASSUMPTION) | **SHA-256 hash** + display prefix + scope + owner + last_used + expiry | constant-time hash compare | no (shown once) |
| Deploy Token | random 32 bytes base64url | **encrypted at rest** (envelope, §8.3) because parity requires reveal/rotate in UI | decrypt-compare or store a second hash for lookup (ASSUMPTION: store hash for lookup + ciphertext for reveal) | yes (needs `secrets` permission) |
| Setup code | random, short-lived | hash in `settings` | constant time | no |
| Invite/reset links | random, 24h, single-use | hash | constant time | no |

High-entropy random tokens → fast hash (SHA-256) is adequate; passwords use argon2id. Tokens never logged (`Secret<T>` wrapper redacts `Debug`/`Display`; `secrecy` crate **unverified**, or hand-rolled).

### 5.5 Webhooks

- `POST /deploy?id=<project>` with `Authorization: Bearer <Deploy Token>` → `deployments.trigger(Actor::DeployToken)`.
- `POST /webhooks/github` → verify `X-Hub-Signature-256` HMAC over the **raw body** (legacy captured raw body for this reason; in axum extract `Bytes` before JSON parsing). Per-Project webhook secret: **ASSUMPTION** (check how legacy stored it before implementing). Only `push` to the Project's tracked branch triggers.

## 6. Background work

All background tasks are spawned by `yanto serve` only, under one `tokio_util::sync::CancellationToken` (crate **unverified**) for graceful shutdown (SIGTERM/SIGINT → stop accepting, cancel/drain Deployments for `shutdown_timeout`, close pool; legacy default 30 s).

### 6.1 Deployment queue (SQLite table)

```
deployments(id, project_id, status, trigger, target_ref, queued_at, started_at,
            finished_at, exit_code, requested_by, cancel_requested, log_bytes, …)
status: queued → running → success | failed | cancelled
```
- **Enqueue** = `INSERT` with `status='queued'` (service, in a txn with audit). Then `Notify::notify_one()` in-process; a **poll fallback** every ~2 s (ASSUMPTION) covers inserts made by other processes (`yanto mcp` stdio, CLI) — SQLite has no cross-process notify.
- **Claim**: single worker loop; `UPDATE deployments SET status='running', started_at=? WHERE id=(SELECT id FROM deployments d WHERE status='queued' AND NOT EXISTS (SELECT 1 FROM deployments r WHERE r.project_id=d.project_id AND r.status='running') ORDER BY queued_at LIMIT 1) RETURNING *`. Guarantees **one running Deployment per Project**; global concurrency via a `Semaphore` (`deploy.max_concurrent`, default 2 — ASSUMPTION).
- **Crash recovery** on startup (legacy step 6): all `running` → `failed` with "Yanto restarted" log line; `queued` stay queued.
- **Cancel**: set `cancel_requested`; worker holds `HashMap<DeploymentId, CancellationToken>`; cancellation kills the running subprocess (§7.2). Stop/rollback/restart use the same machinery (parity: stop, rollback with preview, queue).
- **Timeouts**: `deploy.command_timeout` (legacy: 1 h) per step; `deploy.log_max_bytes` truncation marker (legacy: 500 000 chars).

### 6.2 Log storage and SSE fan-out

- **EventHub** (`yanto-core::jobs::hub`): `DashMap`/`Mutex<HashMap<_, broadcast::Sender<Event>>>` keyed by Deployment id (log lines) plus one global channel (deployment status, container events for the dashboard). `tokio::sync::broadcast` capacity ~1024 (ASSUMPTION).
- Deployment runner appends log lines to `deployment_logs(deployment_id, seq, ts, stream, text)` in batches (every N lines or 250 ms — ASSUMPTION) and publishes `Event::Log{seq,…}` after the batch commits.
- **SSE endpoint** (axum `Sse` + `KeepAlive`, docs-confirmed; wrap `broadcast::Receiver` with `tokio_stream::wrappers::BroadcastStream`, as the axum example comment suggests): **subscribe first, then replay** rows with `seq > Last-Event-ID` from SQLite, then forward live events, dropping duplicates by `seq`. On `Lagged`, re-query from the last delivered `seq` — no data loss, bounded memory. SSE `id:` = `seq`, so browser reconnect resumes automatically.
- htmx side: `hx-ext="sse"` (htmx SSE extension, vendored) swaps log fragments; status badges update from the global channel.

### 6.3 Retention tasks (tokio, interval)

| Task | Default | Notes |
|---|---|---|
| Deployment log pruning | keep last N Deployments per Project and/or age (ASSUMPTION: 50 / 90 d) | deletes `deployment_logs` rows, keeps the `deployments` row |
| Audit log pruning | optional, off by default (ASSUMPTION) | |
| Expired sessions / invites / reset tokens | hourly | |
| Backup pruning | per-schedule `keep` count | files + rows |
| WAL checkpoint | `PRAGMA wal_checkpoint(TRUNCATE)` hourly (ASSUMPTION) | |

Each runs as its own `tokio::spawn` loop with `tokio::select!` on the cancellation token and `MissedTickBehavior::Delay`.

### 6.4 Database setup (sqlx + SQLite)

- `SqliteConnectOptions` with `journal_mode(SqliteJournalMode::Wal)` (docs-confirmed); also `foreign_keys(true)`, `busy_timeout`, `synchronous(Normal)`, `create_if_missing(true)` — these builder options exist in sqlx but were **not** re-verified in this session (ASSUMPTION).
- Pool strategy (ASSUMPTION): one **writer pool of 1 connection** + a **reader pool of N** to avoid `SQLITE_BUSY` write contention; all writes go through the writer. Transactions use `BEGIN IMMEDIATE` where read-then-write.
- Migrations: `sqlx::migrate!("./migrations")` embeds SQL in the binary (docs-confirmed; path relative to the crate with `Cargo.toml`). Run at `serve` start and by `yanto install`. `.gitattributes` forces LF on `*.sql` (docs warn CRLF changes hashes).
- Compile-time-checked queries (`query!`/`query_as!`) with committed `.sqlx/` offline metadata; CI runs `cargo sqlx prepare --check` (doc 05). Migrations are append-only; no down migrations (ASSUMPTION).
- Fresh schema (ADR 0004): no AI tables, no worker/node tables, no `session_version`.

## 7. Host and Docker integration

Everything with side effects on the Host sits behind a port trait in `yanto-core::ports` and an implementation in `yanto-host`. Fakes in `yanto-core::testing`.

### 7.1 Docker API (bollard)

- `bollard::Docker::connect_with_unix_defaults()` / `connect_with_socket_defaults()` (docs-confirmed) to `/var/run/docker.sock`; client is `Clone` and shared in `AppState`. Config may override the socket path (ASSUMPTION).
- Used for: list/inspect/start/stop/restart containers, logs, stats, exec (terminal), image pull with Registry Credential auth, prune with preview (list candidates first), info/disk usage, Hosted Registry container lifecycle.
- **Logs**: `docker.logs(id, Some(LogsOptionsBuilder::default().stdout(true).stderr(true).follow(true).timestamps(true).build()))` returns `Stream<Item = Result<LogOutput, Error>>` (docs-confirmed). Container log streaming to the dashboard reuses the SSE pattern, one stream per open client, dropped on disconnect.
- **Exec terminal**: axum WebSocket ↔ bollard exec attach (stdin/stdout streams). Gated by `runtime` permission, every session opened/closed is audited, with idle timeout (ASSUMPTION: 15 min). Terminal front-end (xterm.js) is vendored as a static file; this does not reintroduce npm (ASSUMPTION: vendoring a pre-built JS file is acceptable under D3).
- bollard version: docs list `v0.20.0` (Context7 version list; verify on crates.io before pinning). Query-parameter builders live in `bollard::query_parameters` in that docs set.

### 7.2 `docker compose` via `tokio::process`

Compose has no daemon API, so Yanto shells out to the `docker compose` plugin (prereq, doc 05).

- Invocation: `docker compose --project-name <slug> --project-directory <dir> -f <compose file(s)> [--env-file <f>] <verb>`; verbs: `config` (validate/preview), `pull`, `up -d --remove-orphans`, `ps --format json`, `down`, `exec -T` (DB dumps). Arguments are passed as `Command::args` (never via a shell); Project names are slugged `[a-z0-9_-]`.
- **Streaming**: `Command::new("docker").stdin(null).stdout(piped).stderr(piped).kill_on_drop(true)`; two reader tasks (`BufReader::lines` or chunked reads, tolerant of `\r` progress output) push into one `mpsc` to the log sink (DB batch writer + EventHub). Output is size-capped by `deploy.log_max_bytes`.
- **Cancellation**: spawn the child in its **own process group** (`std::os::unix::process::CommandExt::process_group(0)` — stable std API), then on cancel/timeout send `SIGTERM` to the group, wait `grace` (10 s — ASSUMPTION), then `SIGKILL`. Required because `docker compose` spawns children (buildkit, git). Signal sending through `nix` (**unverified**) or `libc::kill` wrapper.
- **Git** (`git.rs`): `git` CLI, not a Rust git library (ASSUMPTION: matches legacy and honors host config). Deploy Key via `GIT_SSH_COMMAND="ssh -i <key> -o IdentitiesOnly=yes -o StrictHostKeyChecking=accept-new"` (legacy default `accept-new`). Same cancellation path.
- Working copies live under `/var/lib/yanto/projects/<slug>/`; image-backed Projects store only generated compose + `.env`.

### 7.3 Host integration modules (one concern each)

All file writers share `hostfs.rs`: write to a temp file in the same directory → `fsync` → set mode/owner → `rename` (atomic); take an `flock` (via `fs4`/`nix`, **unverified**) around read-modify-write; refuse symlink targets; validate input before touching disk; never include newlines from user input.

| Module | Concern | Behavior |
|---|---|---|
| `cron.rs` | **Cron Entries** → `/etc/cron.d/yanto` (D11) | Yanto owns the whole file. Rendered from DB: header (`# Managed by Yanto — do not edit`, `SHELL=/bin/sh`, `PATH=…`), then `m h dom mon dow <user> <command>`. Validates cron expression fields, that `<user>` exists in `/etc/passwd`, no newline/`%` surprises (cron treats `%` specially; escape — ASSUMPTION). File `root:root 0644` (cron ignores files writable by group/other). Temp file must not be picked up by cron: Debian-family cron skips names containing `.`, so use a dotted temp name (**ASSUMPTION**; verify on Debian 12/Ubuntu 22.04). Read-only listing of `/etc/crontab` and other `/etc/cron.d/*` files. Backup schedules are Cron Entries calling `yanto backup …`. |
| `authorized_keys.rs` | **Authorized Keys** (D12) | Edits only the block between `# BEGIN YANTO MANAGED KEYS` / `# END YANTO MANAGED KEYS` (marker text ASSUMPTION) in `~<user>/.ssh/authorized_keys` for the configured Unix user (`ssh.authorized_keys_user`); content outside the block is byte-preserved. Public keys validated by parsing (`ssh-key` crate, **unverified**): key type allow-list, no options prefix, single line. Ensures `~/.ssh` `0700`, file `0600`, owner = that user. Missing block is appended; if the markers are malformed (one missing/duplicate) the op **fails** rather than guessing. |
| `systemd.rs` | **Host Daemon** checks | `systemctl is-active <unit>`, `is-enabled`, and `show -p ActiveState,SubState,MainPID <unit>` via `tokio::process` (no D-Bus dependency; ASSUMPTION vs `zbus`). Binary presence via `PATH` lookup. Returns `DaemonStatus { installed, enabled, active, detail }`. Yanto may `restart`/`reload` **only** units on a fixed allow-list (`cloudflared`, `frps`, `traefik`, configurable unit names). Never installs. Returns install instructions text per daemon. |
| `ingress/traefik.rs` | Traefik Ingress Provider | Writes a Yanto-owned file in Traefik's **file-provider directory** (`ingress.traefik.dynamic_dir`, e.g. `/etc/traefik/dynamic/` — path ASSUMPTION, operator-configured) as `yanto.yml`: one `http.routers.<id>` + `http.services.<id>.loadBalancer.servers[0].url = http://127.0.0.1:<port>` per Hostname, `tls.certResolver` from config (`ingress.traefik.cert_resolver`). Atomic write; Traefik's file provider with `watch: true` reloads without restart. Check verifies: daemon active, file provider dir exists, and (best effort) that ports 80/443 are bound. Static Traefik config remains the operator's. |
| `ingress/cloudflare_tunnel.rs` | Cloudflare Tunnel Ingress Provider | Remote-managed (ADR 0006): updates the tunnel's ingress configuration through the Cloudflare API (`cloudflare_api.rs`); host `cloudflared` runs with the tunnel token from a unit the operator set up. Yanto checks `cloudflared` daemon status and tunnel health from the API. Targets are `http://127.0.0.1:<port>`. |
| `cloudflare_api.rs` | Cloudflare clients/zones/tunnels/DNS/diagnostics | `reqwest` JSON client; API tokens stored encrypted (§8.3); DNS record management independent of provider (ADR 0008). |
| `frps.rs` | **FRP Tunnel** server side (D17) | Reads/writes `frps` config (`frp.config_path`, e.g. `/etc/frp/frps.toml` — ASSUMPTION), checks daemon, lists proxies/tunnels via frps dashboard API (needs `webServer` enabled in the operator's config; Yanto shows the required snippet). Applying config = `systemctl restart frps`, which drops live tunnels — flagged as a UI confirm. ASSUMPTION: frps has no hot reload. |
| `registry.rs` | **Hosted Registry** (D10) | Container `registry:2` started via bollard on `127.0.0.1:5000`, mounts `/var/lib/yanto/registry/{data,auth}`, `REGISTRY_AUTH=htpasswd`; htpasswd entries use bcrypt (registry requirement; `bcrypt` crate **unverified**). Repo/tag browse and tag delete use the registry v2 HTTP API (`/v2/_catalog`, `/v2/<name>/tags/list`, manifest `DELETE` — needs `REGISTRY_STORAGE_DELETE_ENABLED=true`, ASSUMPTION from registry docs not checked here). GC = `docker exec <registry> registry garbage-collect` (ASSUMPTION; requires brief read-only mode or accepted risk). Exposure to the internet is a normal Hostname. |

### 7.4 Ingress Provider trait

```rust
#[async_trait]   // object-safe; two providers today, registry keyed by kind
pub trait IngressProvider: Send + Sync {
    fn kind(&self) -> IngressKind;                         // CloudflareTunnel | Traefik
    async fn check(&self) -> ProviderStatus;               // daemon + config sanity, never mutates
    async fn plan(&self, desired: &[HostnameBinding]) -> Result<IngressPlan, IngressError>; // diff, for preview
    async fn apply(&self, desired: &[HostnameBinding]) -> Result<ApplyReport, IngressError>; // idempotent, full desired state
    async fn diagnose(&self, h: &HostnameBinding) -> Vec<Diagnostic>; // DNS, port reachable, cert/tunnel state
}
pub struct HostnameBinding { pub hostname: Hostname, pub target_port: u16 /* 127.0.0.1 */, pub project: ProjectId, pub tls: TlsMode }
```
- **Declarative**: `apply` receives the full desired set for that provider (from DB), so file/API state is always regenerated, never patched (keeps drift impossible and makes retries safe).
- A Hostname row carries `provider` (enum) — a Project may mix providers across Hostnames. DNS is a separate `DnsPort` call chosen independently (ADR 0008).
- `async-trait` crate (**unverified** version) used because the tower-sessions docs also use it and dyn dispatch is wanted; native async-fn-in-trait is not object-safe. ASSUMPTION: acceptable; could switch to an enum if only two providers ever exist.
- Port collision detection (parity "Change"): `projects.declared_ports` unique across Projects at the service layer + a live check of listening sockets before apply.

## 8. Configuration and data layout

### 8.1 Config file

`/etc/yanto/config.toml`, `root:root 0600`, written by `yanto install` (doc 05), read at startup via `serde` + `toml` (**unverified**). Override path with `--config` / `YANTO_CONFIG`. Env overrides: `YANTO_<SECTION>__<KEY>` only for a documented few (bind, port, log level) — ASSUMPTION. Config is **static bootstrap**; operator-editable runtime settings (Cloudflare accounts, SSH user, app URL, command timeouts) live in the DB `settings` table and the dashboard, as in legacy.

```toml
# illustrative — no secrets in this file; they live in the data dir and DB
config_version = 1
[server]
bind = "127.0.0.1"
port = 9197
public_url = ""              # optional; drives cookie Secure + CSRF origin check

[paths]
data_dir = "/var/lib/yanto"

[ssh]
authorized_keys_user = "deploy"   # Unix user for Authorized Keys

[docker]
socket = "/var/run/docker.sock"

[registry]
enabled = false

[ingress]
providers = ["cloudflare-tunnel", "traefik"]   # which to expect/check
[ingress.traefik]
dynamic_dir = "/etc/traefik/dynamic"
cert_resolver = "letsencrypt"
[ingress.cloudflared]
unit = "cloudflared"

[frp]
enabled = false
config_path = "/etc/frp/frps.toml"
unit = "frps"

[deploy]
max_concurrent = 2
command_timeout = "1h"
log_max_bytes = 500000

[log]
level = "info"
format = "json"     # or "pretty"
```

The file never contains the app encryption key, tokens, or passwords.

### 8.2 Data dir `/var/lib/yanto` (root, `0700`)

```
/var/lib/yanto/
  yanto.db  yanto.db-wal  yanto.db-shm     # SQLite (WAL)
  keys/app.key                              # 32-byte envelope key, 0600, generated at install
  ssh/<project-slug>/id_ed25519{,.pub}      # Deploy Keys, dir 0700, private 0600
  projects/<project-slug>/                  # git checkout or generated compose + .env
  backups/
    projects/<project-slug>/<ts>-<kind>.tar.zst|.sql.gz
    yanto/<ts>-yanto.db                     # SQLite online backup
  registry/{data/,auth/htpasswd}            # Hosted Registry storage + htpasswd (0600)
  tmp/                                      # scratch, wiped on start
```
Outside the data dir (all documented, all under Yanto's control): `/etc/yanto/config.toml`, `/etc/cron.d/yanto`, the managed block in the configured user's `authorized_keys`, Yanto's Traefik file, the `frps` config, the systemd unit `/etc/systemd/system/yanto.service`.

### 8.3 Secrets at rest (ASSUMPTION — design not in 00-scope)

Reveal/rotate requirements (Deploy Token, `.env` secrets, Registry Credentials, Cloudflare API tokens) need recoverable secrets. Use envelope encryption with `keys/app.key` and an AEAD (`chacha20poly1305` or `aes-gcm`, **unverified**), ciphertext + nonce in SQLite. Threat model matches ADR 0007: the Host and root are trusted; this protects DB backups and downloaded Backups of Yanto's SQLite from leaking secrets. `app.key` is **excluded** from Backups of Yanto's database (and noted in restore docs: restoring the DB on another Host without the key loses secrets). Open question #2.

## 9. Error handling and tracing

- Libraries use `thiserror`; `anyhow` only in `yanto-cli` and test code.
- `CoreError { kind: ErrorKind, message, source }`; `ErrorKind` = `Validation(Vec<FieldError>) | Unauthenticated | Forbidden | NotFound | Conflict | Unavailable (Host Daemon down, Docker unreachable) | RateLimited | Internal`. Internal errors keep the source chain for logs and show an opaque message + request id to callers (legacy masked 500s similarly).
- Adapter mapping: REST → `{ "error": { "code", "message", "fields?", "request_id" } }` with 400/401/403/404/409/429/503/500; dashboard → full error page or, for htmx requests, an error fragment with `HX-Retarget`/`HX-Reswap` toast; MCP → tool result with `isError` and the same message (protocol-level `ErrorData` only for protocol errors). Validation errors are field-keyed so all three render the same facts.
- `tracing` + `tracing-subscriber` (versions **unverified**): `EnvFilter` from `log.level`/`RUST_LOG`; JSON to stdout so journald captures it (`journalctl -u yanto`); a request span with `request_id`, actor kind, route; a Deployment span with `deployment_id`, `project`. Secrets never enter spans (`Secret<T>` redaction; subprocess argv logged with env values omitted).
- "App logs" view (parity: System) reads an in-memory ring buffer `tracing` layer (last 500 lines, as legacy) — ASSUMPTION vs shelling out to `journalctl`.
- Panics: `panic = "abort"` is **not** used (a handler panic should not kill the queue); `tokio` task panics are caught at spawn boundaries and logged.

## 10. Testing strategy

| Tier | What | Notes |
|---|---|---|
| Unit | Pure logic: cron-line rendering/validation, authorized_keys block surgery, Traefik YAML rendering, permission matrix, token scope mapping, config parsing | In-crate `#[cfg(test)]`; golden files for rendered host files; `proptest` (**unverified**) optional for cron/authorized_keys parsers |
| Repository/service | Services against real SQLite with fakes for every port | `#[sqlx::test]`: docs confirm SQLite works without `DATABASE_URL` (per-test DB at `target/sqlx/test-dbs/…`), requires `macros`+`migrate` features, supports `fixtures(...)` and `migrations = "…"`, failed-test DBs are kept for debugging |
| Adapter | Axum routers driven via `tower::ServiceExt::oneshot` with a test `AppState` (fake ports); session+CSRF flow, token scopes, REST error mapping, htmx fragment vs full page, SSE replay/dedupe, MCP tool calls through rmcp's in-process transport | no network, no Docker |
| Host-file integration | Real file writers against a `tempdir` root (injectable base path, so `/etc` is never touched) | runs in normal CI |
| Docker-dependent | Real bollard + `docker compose` + git against a local daemon; deploy a tiny compose Project, cancel mid-run, exec, registry enable/push | `#[ignore]` + feature `docker-tests` (or env `YANTO_DOCKER_TESTS=1`); run in a dedicated CI job on a Linux runner with Docker; never in default `cargo test` |
| Install smoke | `yanto install --non-interactive` in a systemd-capable container/VM, then `curl` the health endpoint | release-gate job, not per-PR (ASSUMPTION) |

Coverage target not set. CI wiring in doc 05.

## 11. Recommended dependencies

All versions **unverified** unless noted; pin at first `cargo add` after checking crates.io. "Docs" = confirmed against Context7 docs this session.

| Area | Crate | Why / notes |
|---|---|---|
| Runtime | `tokio` (+ `tokio-util`, `tokio-stream`) | process, sync::broadcast, time, signal; `tokio-stream` for `BroadcastStream` |
| HTTP | `axum` (docs list tag `axum_v0_8_4`; verify), `tower`, `tower-http` | routing, extractors, `Sse`+`KeepAlive`, `with_state`, WebSocket; tower-http for trace/limit/compression |
| Sessions | `tower-sessions` + custom sqlx store | docs-confirmed `SessionStore` trait and `SessionManagerLayer::with_expiry`; custom store for per-user revocation |
| Templates | **`askama`** (docs list v0.16.1 / v0.14.0; verify) | compile-time-checked, Jinja-style `{% extends %}`/blocks → layout + partials for htmx; errors at build time, zero runtime template files to ship; axum example exists. Chosen over `maud` (ASSUMPTION: maud not evaluated in docs; its HTML-in-Rust style makes designer-style edits and partial reuse harder) |
| Assets | `rust-embed` or `include_dir` | embed htmx, SSE ext, CSS, xterm into the binary; pick at implementation |
| DB | `sqlx` (features `sqlite`, `runtime-tokio`, `migrate`, `macros`; Context7 lists v0.9.0/v0.8.6 from a Medium-reputation source — verify) | compile-time queries, embedded migrations, `sqlx::test` |
| MCP | `rmcp` (features `server`, `macros`, `transport-streamable-http-server`, `transport-io` — feature names unverified) + `schemars` | docs: `StreamableHttpService`, `stdio`, `#[tool_router]` |
| Docker | `bollard` (docs list `v0.20.0`; verify) | API client; `futures-util` for streams |
| CLI | `clap` v4 with `derive`, `env` features (docs list v4.6.7; verify) | `#[command(subcommand)]`, `#[arg(global = true)]`, `Arg::env` (docs-confirmed) |
| Prompts | **`inquire`** (version unverified) | see below |
| Serde | `serde`, `serde_json`, `toml`, `serde_yaml`-successor for Traefik YAML (**unverified which crate is maintained**; alternative: render YAML via askama/string templates + validate by round-trip) | |
| Crypto | `argon2`, `sha2`, `hmac`, `subtle`, `rand`, `chacha20poly1305` or `aes-gcm`, `bcrypt` (htpasswd) | |
| SSH keys | `ssh-key` | generate ed25519 Deploy Keys, parse/validate Authorized Keys |
| HTTP client | `reqwest` (rustls, no OpenSSL — required for musl static builds) | Cloudflare API, registry API, GitHub release check for `self-update` |
| Errors/log | `thiserror`, `anyhow`, `tracing`, `tracing-subscriber` | |
| Misc | `time`, `uuid` or `ulid` (ids), `bytes`, `async-trait`, `nix` (signals, chown), `fs4` (flock), `sysinfo` (CPU/mem/disk for System page), `zip`/`tar`+`zstd`/`flate2` (backups) | |
| Dev | `tempfile`, `insta` (golden), `wiremock` (Cloudflare/GitHub fakes), `proptest` | |

**Prompt crate choice — `inquire` over `dialoguer`:** docs show `inquire` offers built-in `Confirm` (with defaults/help text), `Select`, `Text`, `Password` (hidden input, confirmation prompt, validators) and a `Validation::{Valid, Invalid}` validator API plus `prompt_skippable` and error enum for Ctrl-C/Esc handling; that covers every wizard prompt in doc 05 with validation inline (port range, existing Unix user, absolute path). `dialoguer` was not evaluated via docs (ASSUMPTION it is comparable but offers less built-in validation ergonomics). The wizard lives only in `yanto-cli`, so the choice is easy to swap.

**musl caveat (ASSUMPTION):** target `*-unknown-linux-musl` static; avoid OpenSSL (use rustls), avoid crates needing system C libs. SQLite is bundled by `libsqlite3-sys` (`bundled` feature; needs a C compiler for musl cross-builds — doc 05). The musl allocator is slow under contention; consider `mimalloc`/`jemalloc` only if profiling demands it.

## 12. Open questions and risks

1. **`rmcp` stability** (ADR 0005 already allows dropping HTTP). Version and exact feature flags unverified; isolate in `yanto-server::mcp`.
2. **Secrets at rest design** (§8.3) is new — confirm key handling, exclusion of `app.key` from Backups, and what restore means across Hosts.
3. **Stdio MCP as a separate process** shares SQLite with `serve`; Deployments it enqueues rely on the poll fallback. Alternative: stdio proxies to the local REST API.
4. **frps apply = restart** drops tunnels; need confirm UX or a hot-reload check in frps docs.
5. **Volume Backups**: method not decided (helper container via bollard vs `docker run … tar`); needs an image available offline.
6. **Cron `%` escaping and temp-file naming** on Debian/Ubuntu need a real test.
7. **Exec terminal over WebSocket** is the largest attack surface of the dashboard; session + CSRF + Origin + permission + audit are all required, plus idle timeout.
8. **GitHub webhook secret storage** unknown from legacy docs — check source before implementing.
9. **sqlx SQLite single-writer** pool design is an ASSUMPTION; load is small (ADR 0002) but queue claim and log batch writes must not starve interactive requests.
10. **vendored JS** (htmx, SSE ext, xterm) needs a documented update procedure and checksum list to stay compatible with "no npm".
