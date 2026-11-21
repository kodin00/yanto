> Classification: CONFIDENTIAL

# 03 — Interfaces: auth, dashboard, REST, MCP, CLI

Binding inputs: [`00-scope.md`](00-scope.md), ADR 0003/0005/0008, [`CONTEXT.md`](../../CONTEXT.md), [`02-data-model.md`](02-data-model.md). Legacy reference: `docs/legacy/02-auth-and-rbac.md`, `04-api-reference.md`, `08-mcp-server.md`.

Conventions: unlabeled behavior is ported from legacy. **ASSUMPTION** marks anything new or not grounded in legacy code or ADRs. Crate/library behavior (`axum`, `rmcp`, tower middleware) is **unverified** (not checked against Context7 in this pass).

**Alignment rule.** Every operation has one *service operation* name (`snake_case`, `noun_verb`). The dashboard handler, the REST row, and the MCP tool all call that operation. MCP tool name = `yanto_` + operation name. Section (c) is the master table; section (d) lists only MCP-specific facts (confirm, exclusions).

## a) Auth and RBAC

### a.1 Principals and roles

| Principal | Credential | Surfaces | Authorization model |
|---|---|---|---|
| Owner | password, session cookie | dashboard | Everything. Bypasses Project access checks. Exactly one (DB-enforced). |
| Member | password, session cookie | dashboard | Only Projects granted in `user_project_access`, with per-Project permissions. Never reaches Owner-only operations. |
| API Token | `Authorization: Bearer yat_...` | `/api/v1`, `/mcp`, `yanto mcp` (stdio) | Scope `read` < `write` < `admin`; a higher scope includes lower. **Not Project-scoped**: acts with Owner-level reach bounded by scope (legacy MCP tokens behaved this way). Open question 1. |
| Deploy Token | `Authorization: Bearer <token>` on `POST /deploy` | that one endpoint | Can only trigger a Deployment of its own Project. |
| GitHub | `X-Hub-Signature-256` | `POST /webhooks/github` | Can only trigger a `push`-matched Deployment of its own Project. |
| Host root (CLI) | local process as root | `yanto ...` | Trusted (ADR 0007). CLI writes the same audit log (`actor_type='cli'`). |

Cookie credentials are accepted only by dashboard routes; Bearer credentials only by `/api/v1`, `/mcp`, `/deploy`. A cookie sent to `/api/v1` is ignored. This removes CSRF from the API entirely (risk area: access control / session management) (ASSUMPTION: legacy accepted cookies on `/api/*`).

### a.2 Project permissions

Permissions: `deploy`, `runtime`, `config`, `secrets`, `backups`, `hostnames`. Having a `user_project_access` row (even with no permissions) grants *view* of that Project, its Deployments, containers, logs, and audit entries. Missing Project or missing access returns **404** (hides existence); access without the specific permission returns **403** (legacy). Owner-only operations return 403 for Members.

| Permission | Allows (Member, on that Project) |
|---|---|
| (view only) | `project_get/list`, `deployments_list`, `deployment_get/logs`, `deployment_logs_stream`, `containers_list`, `container_logs(_stream)`, `audit_list` (Project-scoped), `project_ports_list`, `project_databases_list`, `hostnames_list` (names visible only with `hostnames`; see below) |
| `deploy` | `project_deploy` (without env), `project_rollback_preview`, `project_rollback`, `deployment_cancel` |
| `runtime` | `project_stop`, `project_restart`, `container_start/stop/restart`, `container_exec` (dashboard terminal + REST) |
| `config` | `project_update` (not `slug`, `source_kind`), `project_compose_read`, `project_ports_set`, `project_databases_set`, assign `deploy_key_id`/`registry_credential_id` (select among existing ones only) |
| `secrets` | `project_env_read`, `project_env_content_read`, `project_env_preview`, `project_env_write`, `project_env_variables_write`, `project_deploy_token_reveal`, `project_deploy_token_rotate`, `project_deploy` **with** env payload (also needs `deploy`), `deploy_key_public_get` |
| `backups` | `backups_list`, `backup_create`, `backup_download`, `backup_delete` for that Project's volumes/DBs; adds the `backups` view |
| `hostnames` | `hostnames_list`, `hostname_create/delete/retry/enable/disable`, `hostnames_diagnostics` for that Project; adds the `hostnames` view. Members choose only the provider/tunnel/zone already configured by the Owner. |

Owner-only (Member always 403): `project_create`, `project_delete`, all `user_*`, `api_token_*`, `settings_*`, `registry_*`, `registry_credential_*`, `deploy_key_*` (create/import/delete), `cron_*`, `authorized_keys_*`, `cloudflare_*` (clients, zones, tunnels, DNS), `frps_*`, `daemon_*`, `traefik_status`, `system_*`, `backup_create` of `yanto_sqlite`, `backup_restore`, `audit_list` (global).

Container access: a Docker container maps to a Project by the Compose label `com.docker.compose.project == projects.slug`. `slug` is UNIQUE, so mapping is unambiguous. Containers without a Project label are Owner-only.

Derived app views (not stored, doc 02): Owner = `dashboard, projects, deployments, containers, backups, hostnames, registry, cron, ssh, frp, dns, audit, settings, system`. Member = `dashboard, projects, deployments, containers, audit` + `backups` / `hostnames` if any granted Project carries that permission. (`tasks`, `nodes` removed; `registry`, `cron`, `ssh`, `system` are new Owner views.)

### a.3 Session and CSRF rules (dashboard)

| Rule | Value |
|---|---|
| Session | Opaque 32-byte random id; cookie `__Host-yanto_session` when `cookie_secure`, else `yanto_session` (`__Host-` requires `Secure`; ASSUMPTION). `HttpOnly`, `SameSite=Lax`, `Path=/`. Server stores SHA-256 in `sessions` (doc 02). |
| Lifetime | 7 days absolute (legacy parity), no sliding window (ASSUMPTION; open question 5). |
| Fixation | New session id on login and account setup; old ones for that browser dropped. |
| Revocation | Delete `sessions` row. Triggers: logout, password change/reset, user disabled/deleted, `owner-reset`, Owner "sign out everywhere". Checked on every request (indexed lookup). |
| Streams | SSE/WebSocket handlers re-check the session row (and the Project permission) every **15 s**; on loss they send a terminal `authorization_revoked` event and close (legacy). |
| CSRF token | Per-session synchronizer token (`sessions.csrf_token`) rendered into a `<meta name="csrf-token">`; htmx sends it through `hx-headers` as `X-CSRF-Token`. Required on every non-GET/HEAD/OPTIONS dashboard request, compared in constant time; failure is 403. **Deviation from legacy:** no per-request rotation, because rotating on each mutation races under concurrent htmx requests (ASSUMPTION). |
| Origin check | Mutating requests must have `Origin` (or `Referer`) host equal to the request `Host` or `app_base_url`, and `Sec-Fetch-Site` not `cross-site` when present. Same check is mandatory on the WebSocket upgrade for container exec (cross-site WebSocket hijacking). |
| CSRF-exempt | `POST /deploy`, `POST /webhooks/github` (own credentials), `/api/v1/*`, `/mcp` (Bearer only). Nothing else. |
| Headers | `Content-Security-Policy` strict (self + nonce for inline; no external origins), `X-Content-Type-Options: nosniff`, `Referrer-Policy: same-origin`, `X-Frame-Options: DENY` / `frame-ancestors 'none'`, `Cache-Control: no-store` on pages that render secrets. |
| First Owner | Dashboard shows setup only while zero users exist. Requires the **one-time setup code** (D18; printed by `yanto install`, stored hashed in `app_settings.setup.code_hash`, constant-time compare, deleted after use). Setup is single-flight (transactional insert against the single-owner index). |
| Passwords | Min 12 chars (ASSUMPTION; legacy minimum not documented), max 1024; Argon2id (ASSUMPTION); constant-time verify; unknown-user logins still burn a dummy hash to avoid a user-enumeration timing oracle. Generic message "Invalid username or password". |
| Account tokens | Invite/reset URL `<app_base_url>/account/setup#token=<token>` (fragment keeps it out of logs and Referer). 24h, single-use; preview via `POST` (not GET). |

### a.4 Rate limiting

In-memory token buckets (ASSUMPTION: no DB persistence; limits reset on restart, acceptable for a loopback-first service). Client IP comes from the socket peer; `X-Forwarded-For` / `CF-Connecting-IP` are honored only if the peer is in `runtime.trusted_proxies` (risk area: input validation; otherwise IPs are spoofable and limits bypassable).

| Endpoint class | Limit (legacy) | Key (new) |
|---|---|---|
| Login | 5 failed / 15 min, successes not counted | IP **and** lowercased username (two buckets; either trips). Lockout returns 429 with `Retry-After`; it does not disable the account. |
| Setup (first Owner, account setup/preview) | 10 / 15 min | IP |
| `POST /deploy` | 10 / 15 min | IP + Project id |
| `POST /webhooks/github` | 60 / min | Project id (and IP) |
| Bearer auth failures (`/api/v1`, `/mcp`) | 20 failed / 15 min | IP |
| Container exec (dashboard) | ASSUMPTION: 30 commands / min | session |

### a.5 API Token scopes mapped to operations

| Scope | Includes | Operation classes |
|---|---|---|
| `read` | — | Non-mutating reads; env reads are **masked**; no secret reveal; no exec; no log of secrets. Also `whoami`, MCP resources. |
| `write` | `read` | Reversible/day-to-day mutations: `project_create/update`, env writes, `project_deploy`, `deployment_cancel`, `container_start`, `backup_create`, `hostname_*` create/retry/enable/disable, `cloudflare_tunnel_create`, DNS create/update, `cron_toggle`, `registry_user_create`, `project_ports_set`, `project_databases_set`, `frps_settings_update`. |
| `admin` | `write` | Destructive, host-level, credential, and reveal operations: all `*_delete`, `project_stop/restart`, `project_rollback`, `container_stop/restart/exec`, `project_deploy_token_reveal/rotate`, credential create/update (Cloudflare, Registry Credential, Deploy Key), `cron_create/update`, `authorized_keys_add`, `registry_enable/disable`, `registry_gc`, `daemon_control`, `settings_update`, `system_cleanup`, `backup_download`, `backup_restore`, `user_*`. |

Rule: a token can never exceed what an Owner can do, and token **issuance, listing, and revocation are not available through any token** (REST/MCP); they are dashboard- or CLI-only (privilege-laundering guard, ASSUMPTION; legacy created tokens only via the Owner session too). Scope check happens in the service layer, not by hiding tools, but MCP additionally hides tools above the token scope from `tools/list` (legacy parity).

### a.6 Deploy Token and GitHub webhook verification

`POST /deploy?id=<projectId>`
1. Rate-limit check. Load Project by id; unknown id returns 401 (same as bad token; do not reveal existence).
2. Require `Authorization: Bearer <token>`; compute SHA-256, compare with `projects.deploy_token_hash` in constant time.
3. Reject 403 if `manual_deploy_enabled=0`.
4. Enqueue Deployment (`trigger='deploy_token'`); 202 `{deployment, reused}`. Body ignored (no ref/env accepted via Deploy Token; ASSUMPTION: narrower than REST).

`POST /webhooks/github?id=<projectId>`
1. Read raw body, capped at 1 MiB (ASSUMPTION; legacy cap 1 MB) before parsing; 413 beyond.
2. Require `X-Hub-Signature-256: sha256=<hex>`; compute HMAC-SHA256 over the **raw** body, key = decrypted Deploy Token (`deploy_token_enc`), compare in constant time. Missing/invalid returns 401. Never fall back to `sha1`.
3. `X-GitHub-Event`: `ping` returns 200 `{ok}`; non-`push` returns 202 ignored. `deleted: true` returns 202 ignored. `ref` must equal `refs/heads/<branch>` else 202 ignored. `github_webhook_enabled=0` returns 202 ignored. Image Projects return 202 ignored.
4. Match enqueues `trigger='github'`, `target_ref` unset (branch tip), 202. Replay safety: coalescing (one queued per Project) bounds replay impact; `X-GitHub-Delivery` dedupe (last 100 ids in memory) is ASSUMPTION.
5. Audit `deployment.trigger` with `actor_type='github'`; metadata holds delivery id, ref, sha, never the body.

Rotating the Deploy Token invalidates both the bearer and the webhook secret; the dashboard reminds the operator to update the GitHub webhook secret.

### a.7 Audit events

Every mutating operation writes one `audit_logs` row (doc 02) with actor type/label, IP, Project id when applicable, and metadata free of secret values (metadata stores key *names* and counts, never values). Security-relevant rows are written in the same transaction as the change.

| Area | `action` values |
|---|---|
| Auth | `auth.login`, `auth.login_failed` (username hash only, not the attempted password; unknown usernames recorded as `<unknown>`), `auth.logout`, `auth.rate_limited`, `auth.setup_owner`, `auth.account_setup`, `auth.session_revoke`, `auth.owner_reset` |
| Users | `user.create`, `user.access_update`, `user.status_update`, `user.reset_link`, `user.delete` |
| API Tokens | `api_token.create`, `api_token.revoke`, `api_token.auth_failed` (rate-limited) |
| Projects | `project.create`, `project.update`, `project.delete`, `project.ports_set`, `project.databases_set`, `project.env_write`, `project.env_read` (only unmasked reads; masked reads are not audited), `project.deploy_token_reveal`, `project.deploy_token_rotate`, `project.stop`, `project.restart` |
| Deployments | `deployment.trigger`, `deployment.cancel`, `deployment.rollback`, `deployment.finish` (system) |
| Containers | `container.start/stop/restart`, `container.exec` (command text recorded, truncated to 1 KiB, redacted of known secrets) |
| Keys/creds | `deploy_key.create/import/delete`, `registry_credential.create/update/delete`, `authorized_key.add/delete/force_change` |
| Registry | `registry.enable/disable`, `registry_user.create/delete/password_set`, `registry.tag_delete`, `registry.gc` |
| Cron | `cron.create/update/delete/toggle` (full command recorded) |
| Cloudflare/ingress | `cloudflare_client.create/update/delete`, `cloudflare_tunnel.create/delete/activate/token_reveal`, `cloudflare_dns.create/update/delete`, `hostname.create/delete/retry/enable/disable`, `daemon.control` |
| System | `settings.update`, `system.cleanup`, `backup.create/download/delete/restore`, `migration.apply`, `self_update` |

Audit visibility: Owner sees all; a Member sees entries whose `project_id` is in their grants.

## b) Dashboard route map (htmx)

Server-rendered HTML (Askama, per 01 §11). Full pages return the layout; **fragments** (`/_f/...`) return partials swapped by htmx. A page may also be requested with `HX-Request` and then returns the content block only (`hx-boost`). Static assets embedded under `/static/*` (hashed names, `Cache-Control: immutable`). Auth column: **pub** none, **S** any session, **M** Project access + permission, **O** Owner.

| Route | Method | Kind | Auth | Service op |
|---|---|---|---|---|
| `/setup` | GET, POST | page, action | pub (only while zero users) | `owner_setup` |
| `/login` | GET, POST | page, action | pub | `session_login` |
| `/logout` | POST | action | S | `session_logout` |
| `/account/setup` | GET, POST | page, action | pub (token) | `account_setup` (+ preview POST `/account/setup/preview`) |
| `/` | GET | page (dashboard: usage, recent Deployments, health) | S | `system_usage`, `deployments_list` |
| `/projects` | GET | page | S | `project_list` |
| `/projects/new` | GET, POST | page, action | O | `project_create` |
| `/projects/{id}` | GET | page (tabs: overview, deployments, config, env, ports, hostnames, backups, access) | M | `project_get` |
| `/projects/{id}/settings` | POST | action | M config | `project_update` |
| `/projects/{id}` | DELETE (`hx-delete`) | action | O | `project_delete` |
| `/projects/{id}/deploy` | POST | action | M deploy | `project_deploy` |
| `/projects/{id}/rollback/preview` | POST | fragment | M deploy | `project_rollback_preview` |
| `/projects/{id}/rollback` | POST | action | M deploy | `project_rollback` |
| `/projects/{id}/stop`, `/restart` | POST | action | M runtime | `project_stop`, `project_restart` |
| `/projects/{id}/compose` | GET | fragment (editor) | M config | `project_compose_read` |
| `/projects/{id}/env` | GET, PUT, PATCH | fragment, action | M secrets | `project_env_content_read`, `project_env_write`, `project_env_variables_write` |
| `/projects/{id}/env/preview` | POST | fragment | M secrets | `project_env_preview` |
| `/projects/{id}/deploy-token` | POST (`reveal`, `rotate` sub-paths) | action | M secrets | `project_deploy_token_reveal`, `project_deploy_token_rotate` |
| `/projects/{id}/ports` | GET, PUT | fragment, action | M config | `project_ports_list`, `project_ports_set` |
| `/projects/{id}/databases` | GET, PUT | fragment, action | M config | `project_databases_list`, `project_databases_set` |
| `/projects/{id}/access` | GET, PUT | fragment, action | O | `user_access_update` |
| `/projects/{id}/hostnames` | GET, POST | fragment, action | M hostnames | `hostnames_list`, `hostname_create` |
| `/deployments` | GET | page (paginated; queue shown) | S | `deployments_list` |
| `/deployments/{id}` | GET | page | M view | `deployment_get` |
| `/deployments/{id}/cancel` | POST | action | M deploy | `deployment_cancel` |
| `/deployments/{id}/logs` | GET | text/plain | M view | `deployment_logs` |
| `/deployments/{id}/logs/stream` | GET | **SSE** (`Last-Event-ID` resume) | M view | `deployment_logs_stream` |
| `/_f/deployments/active` | GET | fragment (polled/`sse-swap`) | S | `deployments_list` |
| `/events` | GET | **SSE** (Deployment status changes, filtered to the principal's Projects) | S | `deployments_events_stream` |
| `/containers` | GET | page | S | `containers_list` |
| `/containers/{id}/start`, `/stop`, `/restart` | POST | action | M runtime | `container_start/stop/restart` |
| `/containers/{id}/logs` | GET | page/text | M view | `container_logs` |
| `/containers/{id}/logs/stream` | GET | **SSE** | M view | `container_logs_stream` |
| `/containers/{id}/exec` | GET | page (terminal) | M runtime | — |
| `/containers/{id}/exec/ws` | GET (upgrade) | **WebSocket** (Origin-checked; PTY; idle timeout 15 min, ASSUMPTION; audited at open and per command) | M runtime | `container_exec_session` |
| `/backups` | GET | page | M backups | `backups_list` |
| `/backups` | POST | action | M backups (O for Yanto SQLite) | `backup_create` |
| `/backups/{id}/download` | GET | file (streamed) | M backups | `backup_download` |
| `/backups/{id}` | DELETE | action | M backups | `backup_delete` |
| `/backups/{id}/restore` | POST | action | O | `backup_restore` |
| `/hostnames` | GET | page | M hostnames | `hostnames_list`, `hostnames_diagnostics` |
| `/hostnames/{id}/enable`, `/disable`, `/retry` | POST | action | M hostnames | `hostname_enable/disable/retry` |
| `/hostnames/{id}` | DELETE | action | M hostnames | `hostname_delete` |
| `/registry` | GET | page (status, users, repos) | O | `registry_status` |
| `/registry/enable`, `/disable` | POST | action | O | `registry_enable/disable` |
| `/registry/users` | GET, POST | fragment, action | O | `registry_users_list`, `registry_user_create` |
| `/registry/users/{id}` | DELETE, PUT (`password`) | action | O | `registry_user_delete`, `registry_user_password_set` |
| `/registry/repos`, `/registry/repos/{name}/tags` | GET | fragment | O | `registry_repos_list`, `registry_tags_list` |
| `/registry/repos/{name}/tags/{tag}` | DELETE | action | O | `registry_tag_delete` |
| `/registry/gc` | POST | action (streams result) | O | `registry_gc` |
| `/registry-credentials` | GET, POST; `/{id}` PUT, DELETE | page/action | O | `registry_credential_*` |
| `/deploy-keys` | GET, POST (`generate`, `import`); `/{id}` DELETE | page/action | O | `deploy_key_*` |
| `/cron` | GET, POST; `/{id}` PUT, DELETE; `/{id}/toggle` POST | page/action | O | `cron_*` |
| `/cron/system-files` | GET | fragment (read-only listing of non-Yanto cron files) | O | `cron_files_list` |
| `/ssh` | GET, POST; `/{id}` DELETE | page/action (Authorized Keys + configured Unix user) | O | `authorized_keys_*` |
| `/cloudflare` | GET | page (clients, tunnels, zones) | O | `cloudflare_clients_list` ... |
| `/cloudflare/clients` | POST; `/{id}` PUT, DELETE; `/validate` POST | action | O | `cloudflare_client_*` |
| `/cloudflare/clients/{id}/zones` | GET (`?refresh=1`) | fragment | O | `cloudflare_zones_list` |
| `/cloudflare/tunnels` | GET, POST; `/{id}` DELETE; `/{id}/activate` POST; `/{id}/token/reveal` POST (dashboard only, `Cache-Control: no-store`) | page/action | O | `cloudflare_tunnel_*` |
| `/dns` | GET, POST; `/{recordId}` PUT, DELETE | page/action (per client + zone) | O | `cloudflare_dns_*` |
| `/frp` | GET | page (frps status, config, live tunnel list) | O | `frps_overview` |
| `/frp/settings` | PUT | action | O | `frps_settings_update` |
| `/daemons` | GET | page (cloudflared, frps, traefik: installed/running, install instructions) | O | `daemons_status` |
| `/daemons/{name}/{action}` | POST (`start|stop|restart`) | action | O | `daemon_control` |
| `/audit` | GET | page (filter by Project) | S | `audit_list` |
| `/system` | GET | page (usage, health, app logs, cleanup preview) | O | `system_usage`, `system_logs`, `system_cleanup_preview` |
| `/system/cleanup` | POST | action | O | `system_cleanup` |
| `/system/logs/stream` | GET | **SSE** (Yanto app log tail) | O | `system_logs_stream` |
| `/users` | GET, POST; `/{id}/access` PUT; `/{id}/status` PATCH; `/{id}/reset-link` POST; `/{id}` DELETE | page/action | O | `user_*` |
| `/settings` | GET, POST | page/action (runtime settings; host paths from `config.toml` shown read-only) | O | `settings_read`, `settings_update` |
| `/settings/api-tokens` | GET, POST; `/{id}` DELETE | page/action (token shown once) | O | `api_token_list/create/revoke` |
| `/settings/sessions` | GET; `/{id}` DELETE | page/action | S | `session_list`, `session_revoke` |
| `/settings/password` | POST | action | S | `password_change` |
| `/healthz` | GET | `ok`, minimal, no checks detail | pub | `health` (details only via `/api/v1/health`) |
| `/deploy` | POST | webhook | Deploy Token | `project_deploy` (trigger `deploy_token`) |
| `/webhooks/github` | POST | webhook | HMAC | `project_deploy` (trigger `github`) |

Error handling: htmx 4xx/5xx responses return a fragment with `HX-Retarget` to a toast container; unauthenticated requests get `HX-Redirect: /login` (htmx) or 303 (full page).

## c) REST `/api/v1`

Bearer-only. JSON in/out (`Content-Type: application/json`, body cap 1 MiB; env/compose endpoints 1 MiB; backup restore upload streamed). Error body `{"error":{"code","message"}}`, codes 400/401/403/404/409/413/422/429/500 (ASSUMPTION: envelope differs from legacy `{message}`). Lists use `?limit` (default 50, max 500) and `?cursor`. Scope enforces the matrix in a.5. Where a Member permission is relevant it is listed in a.2; tokens ignore Project permissions. Secrets (tokens, passwords, private keys, `*_enc`) are never returned except where the row says **reveals** (shown once or on explicit reveal).

| Operation | Method | Path | Scope | Summary |
|---|---|---|---|---|
| `whoami` | GET | `/me` | read | Token name, scope, expiry. |
| `health` | GET | `/health` | read | Detailed checks (DB, Docker, daemons, disk). |
| `system_usage` | GET | `/system/usage` | read | CPU, memory, disk. |
| `system_logs` | GET | `/system/logs?tail=` | read | Recent Yanto app log (text). |
| `system_cleanup_preview` | GET | `/system/cleanup/preview` | read | What a protected Docker prune would remove. |
| `system_cleanup` | POST | `/system/cleanup` | admin | Run protected prune. |
| `audit_list` | GET | `/audit?project_id=&limit=&cursor=` | read | Audit entries. |
| `settings_read` | GET | `/settings` | read | Non-secret settings snapshot. |
| `settings_update` | PATCH | `/settings` | admin | Update runtime settings (02 §4.5). Host paths (SSH user, Traefik, `frps`) are `config.toml` and read-only here. |
| `project_list` | GET | `/projects` | read | Projects with container counts. |
| `project_get` | GET | `/projects/{id}` | read | One Project (no secrets). |
| `project_create` | POST | `/projects` | write | Create Git/image Project; **reveals** Deploy Token once. |
| `project_update` | PATCH | `/projects/{id}` | write | Update fields. |
| `project_delete` | DELETE | `/projects/{id}` | admin | Delete Project, stack, directory. |
| `project_compose_read` | GET | `/projects/{id}/compose` | read | Compose content. |
| `project_env_read` | GET | `/projects/{id}/env?env_file=` | read | Variables, values masked. |
| `project_env_content_read` | GET | `/projects/{id}/env/content` | read | `.env` text, masked. |
| `project_env_preview` | POST | `/env/preview` | read | Parse arbitrary env text, no Project. |
| `project_env_write` | PUT | `/projects/{id}/env` | write | Replace env file. |
| `project_env_variables_write` | PATCH | `/projects/{id}/env` | write | Patch variables; masked values preserved. |
| `project_ports_list` | GET | `/projects/{id}/ports` | read | Declared loopback ports. |
| `project_ports_set` | PUT | `/projects/{id}/ports` | write | Replace ports; 409 on collision. |
| `project_databases_list` | GET | `/projects/{id}/databases` | read | Declared Postgres/MySQL services. |
| `project_databases_set` | PUT | `/projects/{id}/databases` | write | Replace declarations. |
| `project_deploy_token_reveal` | POST | `/projects/{id}/deploy-token/reveal` | admin | **Reveals** Deploy Token. POST to avoid caching/logging in GET. |
| `project_deploy_token_rotate` | POST | `/projects/{id}/deploy-token/rotate` | admin | New token, **reveals** once. |
| `project_deploy` | POST | `/projects/{id}/deploy` | write | Enqueue Deployment (`target_ref?`, env payload?). 202 `{deployment, reused}`. |
| `project_rollback_preview` | POST | `/projects/{id}/rollback/preview` | read | Commit/file diff summary. |
| `project_rollback` | POST | `/projects/{id}/rollback` | admin | Enqueue rollback Deployment. |
| `project_stop` | POST | `/projects/{id}/stop` | admin | `docker compose down`/stop. |
| `project_restart` | POST | `/projects/{id}/restart` | admin | Restart stack. |
| `deployments_list` | GET | `/deployments?project_id=&status=` | read | Includes `queued`. |
| `deployment_get` | GET | `/deployments/{id}` | read | One Deployment. |
| `deployment_logs` | GET | `/deployments/{id}/logs?tail=` | read | Logs (text). |
| `deployment_logs_stream` | GET | `/deployments/{id}/logs/stream` | read | SSE, `Last-Event-ID` resume. |
| `deployment_cancel` | POST | `/deployments/{id}/cancel` | write | Cancel queued or running (ASSUMPTION: new; "stop" in parity refers to Project stop, cancel is the Deployment-level equivalent). |
| `containers_list` | GET | `/containers` | read | Docker containers + Project mapping. |
| `container_logs` | GET | `/containers/{id}/logs?tail=` | read | Logs (text). |
| `container_logs_stream` | GET | `/containers/{id}/logs/stream` | read | SSE. |
| `container_start` | POST | `/containers/{id}/start` | write | |
| `container_stop` | POST | `/containers/{id}/stop` | admin | |
| `container_restart` | POST | `/containers/{id}/restart` | admin | |
| `container_exec` | POST | `/containers/{id}/exec` | admin | One-shot command (`{command}`), returns exit code + output. Not exposed over MCP. |
| `backups_list` | GET | `/backups?project_id=` | read | |
| `backup_targets_list` | GET | `/backups/targets` | read | Declared DBs/volumes eligible. |
| `backup_create` | POST | `/backups` | write | `{kind, project_id?, database_id?}`; `yanto_sqlite` allowed. |
| `backup_download` | GET | `/backups/{id}/download` | admin | Streams file (Backups hold customer data; the Yanto DB archive holds `app.key`). |
| `backup_delete` | DELETE | `/backups/{id}` | admin | |
| `backup_restore` | POST | `/backups/{id}/restore` | admin | Restore a DB dump into its declared service (ASSUMPTION: restore parity with legacy `postgres-targets/.../restore`). |
| `hostnames_list` | GET | `/hostnames?project_id=` | read | |
| `hostname_create` | POST | `/hostnames` | write | `{project_id, project_port_id, hostname, ingress_provider, tunnel_id?, zone_id?, manage_dns}`; registry target (`target_kind='registry'`) is Owner/`admin` only. |
| `hostname_delete` | DELETE | `/hostnames/{id}` | admin | Returns warnings (DNS cleanup). |
| `hostname_retry` | POST | `/hostnames/{id}/retry` | write | Re-run publish. |
| `hostname_enable` / `hostname_disable` | POST | `/hostnames/{id}/enable` / `/disable` | write | |
| `hostnames_diagnostics` | GET | `/hostnames/diagnostics` | read | DNS/tunnel/Traefik/reachability classification per Hostname. |
| `deploy_key_list` | GET | `/deploy-keys` | read | Public data only. |
| `deploy_key_public_get` | GET | `/deploy-keys/{id}` | read | Public key + fingerprint. |
| `deploy_key_create` | POST | `/deploy-keys` | admin | Generate ed25519; returns public key. |
| `deploy_key_import` | POST | `/deploy-keys/import` | admin | Private key write-only. |
| `deploy_key_delete` | DELETE | `/deploy-keys/{id}` | admin | 409 if referenced. |
| `registry_credential_list` | GET | `/registry-credentials` | read | Secret never returned. |
| `registry_credential_create` | POST | `/registry-credentials` | admin | Write-only secret. |
| `registry_credential_update` | PATCH | `/registry-credentials/{id}` | admin | |
| `registry_credential_delete` | DELETE | `/registry-credentials/{id}` | admin | 409 if referenced. |
| `registry_status` | GET | `/registry` | read | Enabled, container state, port, disk. |
| `registry_enable` / `registry_disable` | POST | `/registry/enable` / `/disable` | admin | Starts/stops `registry:2` on `127.0.0.1`. |
| `registry_users_list` | GET | `/registry/users` | read | |
| `registry_user_create` | POST | `/registry/users` | write | **Reveals** generated password once. |
| `registry_user_password_set` | PUT | `/registry/users/{id}/password` | write | |
| `registry_user_delete` | DELETE | `/registry/users/{id}` | admin | |
| `registry_repos_list` | GET | `/registry/repos` | read | Catalog. |
| `registry_tags_list` | GET | `/registry/repos/{name}/tags` | read | |
| `registry_tag_delete` | DELETE | `/registry/repos/{name}/tags/{tag}` | admin | Deletes manifest by digest. |
| `registry_gc` | POST | `/registry/gc` | admin | Run garbage collection. |
| `cron_list` | GET | `/cron` | read | Yanto-owned entries. |
| `cron_files_list` | GET | `/cron/system-files` | read | Other cron files, read-only. |
| `cron_create` | POST | `/cron` | admin | `{name, schedule, run_as_user, command, enabled}`. |
| `cron_update` | PATCH | `/cron/{id}` | admin | |
| `cron_toggle` | POST | `/cron/{id}/toggle` | write | Enable/disable only. |
| `cron_delete` | DELETE | `/cron/{id}` | admin | |
| `authorized_keys_list` | GET | `/authorized-keys` | read | |
| `authorized_keys_add` | POST | `/authorized-keys` | admin | |
| `authorized_keys_delete` | DELETE | `/authorized-keys/{id}` | admin | |
| `daemons_status` | GET | `/daemons` | read | cloudflared/frps/traefik installed/running/version + install hints. |
| `daemon_control` | POST | `/daemons/{name}/{action}` | admin | `systemctl start/stop/restart`; `name` in `cloudflared|frps|traefik`. |
| `traefik_status` | GET | `/traefik` | read | Daemon + dynamic-dir + file sync state. |
| `cloudflare_clients_list` | GET | `/cloudflare/clients` | read | |
| `cloudflare_client_validate` | POST | `/cloudflare/clients/validate` | admin | Token write-only. |
| `cloudflare_client_create` | POST | `/cloudflare/clients` | admin | |
| `cloudflare_client_update` | PATCH | `/cloudflare/clients/{id}` | admin | |
| `cloudflare_client_delete` | DELETE | `/cloudflare/clients/{id}` | admin | |
| `cloudflare_zones_list` | GET | `/cloudflare/clients/{id}/zones` | read | `?refresh=true` hits the API. |
| `cloudflare_tunnels_list` | GET | `/cloudflare/tunnels` | read | |
| `cloudflare_tunnel_create` | POST | `/cloudflare/tunnels` | write | Creates remote-managed tunnel; install instructions returned (token only via dashboard reveal). |
| `cloudflare_tunnel_activate` | POST | `/cloudflare/tunnels/{id}/activate` | admin | Marks the tunnel bound to the Host `cloudflared` (ASSUMPTION). |
| `cloudflare_tunnel_status` | GET | `/cloudflare/tunnels/{id}/status` | read | API health + local daemon state. |
| `cloudflare_tunnel_delete` | DELETE | `/cloudflare/tunnels/{id}?force=` | admin | |
| `cloudflare_dns_list` | GET | `/cloudflare/dns?client_id=&zone_id=` | read | |
| `cloudflare_dns_create` | POST | `/cloudflare/dns` | write | |
| `cloudflare_dns_update` | PATCH | `/cloudflare/dns/{id}` | write | |
| `cloudflare_dns_delete` | DELETE | `/cloudflare/dns/{id}` | admin | |
| `frps_overview` | GET | `/frp` | read | Daemon status, config, live proxies/clients (dashboard API). |
| `frps_settings_update` | PATCH | `/frp/settings` | write | `public_host` and the `frps.toml` fields Yanto owns (04 §8.4); applying them restarts `frps` (needs confirmation, 04 §8.4). |
| `user_list` | GET | `/users` | admin | |
| `user_create` | POST | `/users` | admin | Creates Member; **reveals** invite URL once. |
| `user_access_update` | PUT | `/users/{id}/access` | admin | |
| `user_status_update` | PATCH | `/users/{id}/status` | admin | |
| `user_reset_link` | POST | `/users/{id}/reset-link` | admin | **Reveals** reset URL once; kills sessions. |
| `user_delete` | DELETE | `/users/{id}` | admin | |

Not in REST by design: login/logout/setup, `api_token_*`, `session_*`, `cloudflare_tunnel_token_reveal` (dashboard/CLI only; a.5). Webhook endpoints (`/deploy`, `/webhooks/github`) are outside `/api/v1` (a.6).

## d) MCP

Transports: streamable HTTP at `POST /mcp` (stateless per legacy) and `yanto mcp` over stdio, both via the `rmcp` SDK (version unverified; fallback per ADR 0005 is to drop HTTP first). Auth is an API Token. HTTP: `Authorization: Bearer`, Host/Origin allow-list (loopback, `app_base_url`, configured extras; DNS-rebinding guard), 20 failures / 15 min / IP. stdio: token read from `YANTO_API_TOKEN_FILE` (path to a 0600 file) or, less preferred, `YANTO_API_TOKEN` env (ASSUMPTION; never from argv, since argv leaks via `ps`). Tool registration: tools above the token scope are not listed (legacy parity) **and** the service layer re-checks scope per call.

Tool shape: `name = yanto_<operation>`; input JSON Schema derived from the Rust input struct (`schemars`, ASSUMPTION); result is a text block + `structuredContent`; errors become `isError` results. Annotations (`readOnlyHint`, `destructiveHint`, `idempotentHint`, `openWorldHint`) are derived from the class: R = read-only, W = write, D = destructive, E = open-world (calls Cloudflare/registry/GitHub).

`confirm: true` is **required** (else 400 "confirmation required") for every `admin` operation exposed over MCP that is destructive, reveals a secret, or changes host/credential state. Marked **C** below. The confirm flag is a guard against model misfires, not authorization; audit actor is `api_token:<name>`.

| Tool | Operation | Scope | Class | Confirm |
|---|---|---|---|---|
| `yanto_health` | `health` | read | R | |
| `yanto_system_usage` | `system_usage` | read | R | |
| `yanto_system_logs` | `system_logs` | read | R | |
| `yanto_audit_list` | `audit_list` | read | R | |
| `yanto_cleanup_preview` | `system_cleanup_preview` | read | R | |
| `yanto_cleanup` | `system_cleanup` | admin | D | C |
| `yanto_settings_read` | `settings_read` | read | R | |
| `yanto_settings_update` | `settings_update` | admin | W | C |
| `yanto_projects_list` | `project_list` | read | R | |
| `yanto_project_get` | `project_get` | read | R | |
| `yanto_project_create` | `project_create` | write | W | (Deploy Token **redacted** in MCP result; reveal via separate tool) |
| `yanto_project_update` | `project_update` | write | W | |
| `yanto_project_delete` | `project_delete` | admin | D | C |
| `yanto_project_compose_read` | `project_compose_read` | read | R | |
| `yanto_project_env_read` | `project_env_read` | read | R (masked) | |
| `yanto_project_env_content_read` | `project_env_content_read` | read | R (masked) | |
| `yanto_project_env_preview` | `project_env_preview` | read | R | |
| `yanto_project_env_write` | `project_env_write` | write | W | |
| `yanto_project_env_variables_write` | `project_env_variables_write` | write | W | |
| `yanto_project_ports_list` | `project_ports_list` | read | R | |
| `yanto_project_ports_set` | `project_ports_set` | write | W | |
| `yanto_project_databases_list` | `project_databases_list` | read | R | |
| `yanto_project_databases_set` | `project_databases_set` | write | W | |
| `yanto_project_deploy_token_reveal` | `project_deploy_token_reveal` | admin | D | C |
| `yanto_project_deploy_token_rotate` | `project_deploy_token_rotate` | admin | D | C |
| `yanto_project_deploy` | `project_deploy` | write | W,E | |
| `yanto_project_rollback_preview` | `project_rollback_preview` | read | R,E | |
| `yanto_project_rollback` | `project_rollback` | admin | D,E | C |
| `yanto_project_stop` | `project_stop` | admin | D | C |
| `yanto_project_restart` | `project_restart` | admin | D | C |
| `yanto_deployments_list` | `deployments_list` | read | R | |
| `yanto_deployment_get` | `deployment_get` | read | R | |
| `yanto_deployment_logs` | `deployment_logs` | read | R | |
| `yanto_deployment_cancel` | `deployment_cancel` | write | W | |
| `yanto_containers_list` | `containers_list` | read | R | |
| `yanto_container_logs` | `container_logs` | read | R | |
| `yanto_container_start` | `container_start` | write | W | |
| `yanto_container_stop` | `container_stop` | admin | D | C |
| `yanto_container_restart` | `container_restart` | admin | D | C |
| `yanto_backups_list` | `backups_list` | read | R | |
| `yanto_backup_targets_list` | `backup_targets_list` | read | R | |
| `yanto_backup_create` | `backup_create` | write | W | |
| `yanto_backup_delete` | `backup_delete` | admin | D | C |
| `yanto_backup_restore` | `backup_restore` | admin | D | C |
| `yanto_hostnames_list` | `hostnames_list` | read | R | |
| `yanto_hostname_create` | `hostname_create` | write | W,E | |
| `yanto_hostname_retry` | `hostname_retry` | write | W,E | |
| `yanto_hostname_enable` / `yanto_hostname_disable` | `hostname_enable` / `hostname_disable` | write | W,E | |
| `yanto_hostname_delete` | `hostname_delete` | admin | D,E | C |
| `yanto_hostnames_diagnostics` | `hostnames_diagnostics` | read | R,E | |
| `yanto_deploy_key_list` | `deploy_key_list` | read | R | |
| `yanto_deploy_key_public_get` | `deploy_key_public_get` | read | R | |
| `yanto_deploy_key_create` | `deploy_key_create` | admin | W | C |
| `yanto_deploy_key_delete` | `deploy_key_delete` | admin | D | C |
| `yanto_registry_credential_list` | `registry_credential_list` | read | R | |
| `yanto_registry_credential_create` / `_update` | `registry_credential_create` / `_update` | admin | W,E | C |
| `yanto_registry_credential_delete` | `registry_credential_delete` | admin | D | C |
| `yanto_registry_status` | `registry_status` | read | R | |
| `yanto_registry_enable` / `yanto_registry_disable` | `registry_enable` / `registry_disable` | admin | W / D | C |
| `yanto_registry_users_list` | `registry_users_list` | read | R | |
| `yanto_registry_user_create` | `registry_user_create` | write | W | (password **redacted** from MCP result; set it via `registry_user_password_set` or the dashboard) |
| `yanto_registry_user_delete` | `registry_user_delete` | admin | D | C |
| `yanto_registry_repos_list` / `yanto_registry_tags_list` | `registry_repos_list` / `registry_tags_list` | read | R | |
| `yanto_registry_tag_delete` | `registry_tag_delete` | admin | D | C |
| `yanto_registry_gc` | `registry_gc` | admin | D | C |
| `yanto_cron_list` / `yanto_cron_files_list` | `cron_list` / `cron_files_list` | read | R | |
| `yanto_cron_create` / `yanto_cron_update` | `cron_create` / `cron_update` | admin | W | C |
| `yanto_cron_toggle` | `cron_toggle` | write | W | |
| `yanto_cron_delete` | `cron_delete` | admin | D | C |
| `yanto_authorized_keys_list` | `authorized_keys_list` | read | R | |
| `yanto_authorized_keys_add` | `authorized_keys_add` | admin | W | C |
| `yanto_authorized_keys_delete` | `authorized_keys_delete` | admin | D | C |
| `yanto_daemons_status` | `daemons_status` | read | R | |
| `yanto_daemon_control` | `daemon_control` | admin | D | C (stop/restart; `start` still C for uniformity) |
| `yanto_traefik_status` | `traefik_status` | read | R | |
| `yanto_cloudflare_clients_list` | `cloudflare_clients_list` | read | R | |
| `yanto_cloudflare_client_validate` | `cloudflare_client_validate` | admin | R,E | C |
| `yanto_cloudflare_client_create` / `_update` | `cloudflare_client_create` / `_update` | admin | W,E | C |
| `yanto_cloudflare_client_delete` | `cloudflare_client_delete` | admin | D,E | C |
| `yanto_cloudflare_zones_list` | `cloudflare_zones_list` | read | R,E | |
| `yanto_cloudflare_tunnels_list` | `cloudflare_tunnels_list` | read | R | |
| `yanto_cloudflare_tunnel_create` | `cloudflare_tunnel_create` | write | W,E | |
| `yanto_cloudflare_tunnel_activate` | `cloudflare_tunnel_activate` | admin | W | C |
| `yanto_cloudflare_tunnel_status` | `cloudflare_tunnel_status` | read | R,E | |
| `yanto_cloudflare_tunnel_delete` | `cloudflare_tunnel_delete` | admin | D,E | C |
| `yanto_cloudflare_dns_list` | `cloudflare_dns_list` | read | R,E | |
| `yanto_cloudflare_dns_create` / `_update` | `cloudflare_dns_create` / `_update` | write | W,E | |
| `yanto_cloudflare_dns_delete` | `cloudflare_dns_delete` | admin | D,E | C |
| `yanto_frps_overview` | `frps_overview` | read | R | |
| `yanto_frps_settings_update` | `frps_settings_update` | write | W | |

Resources (read scope only; legacy parity, ASSUMPTION for the two new): `yanto://overview`, `yanto://projects/{id}/compose`, `yanto://projects/{id}/env` (masked), `yanto://deployments/{id}/logs`, `yanto://containers/{id}/logs`, `yanto://system/logs`, `yanto://registry/overview`, `yanto://hostnames`.

Legacy tools **dropped** (no equivalent): `yanto_nodes_list`, `yanto_worker_join_command_reveal`, `yanto_settings_multi_node_update` (multi-node); `yanto_backup_upload_r2`, `yanto_settings_r2_update` (R2); `yanto_settings_cloudflare_update/validate`, legacy global `yanto_cloudflare_dns_*` blob (replaced by per-client DNS), `yanto_cloudflare_assignment*` (Docker-network assignments), `yanto_cloudflare_tunnel_node_status/start/stop/restart` (replaced by `daemon_control` + `cloudflare_tunnel_status`), `yanto_project_cloudflare_route*`/`yanto_cloudflare_route_*`/`yanto_cloudflare_routes_diagnostics` (replaced by `hostname_*`), `yanto_settings_ssh_key_save/generate` (replaced by `deploy_key_*`), `yanto_settings_setup_state_update` (wizard moves to `yanto install`), `yanto_frp_server_start/stop/restart` (replaced by `daemon_control` with `frps`), `yanto_frp_tunnel_create/update/delete` (tunnel definitions dropped), `yanto_backups_postgres_targets_list` (replaced by `backup_targets_list`).

Deliberately **not** exposed over MCP (risk area: access control; an LLM-driven client should not hold these even with `admin`): `container_exec`, `user_*`, `api_token_*`, `session_*`, `backup_download` (binary), SSE streams, `deployment_logs_stream` (clients poll `deployment_logs`).

Prompt-injection note (risk area: input validation): deployment/container logs and env previews returned to an LLM client are untrusted data; tool results label them as such in the text block, and masked env values are never unmasked by MCP.

## e) CLI (names only; [05 §10](05-install-and-ops.md) owns flags and details)

Binary `yanto`. The CLI runs on the Host as root, opens the same SQLite DB and calls the same service layer in-process (ASSUMPTION; avoids needing a token for local admin). Every command that mutates writes audit with `actor_type='cli'`.

| Command | Purpose |
|---|---|
| `yanto serve` | Run the dashboard/REST/MCP server (systemd `ExecStart`). |
| `yanto install` / `yanto uninstall` | Interactive wizard (every prompt has a flag, `--non-interactive`, D19) / removal. `install` creates the data dir, `keys/app.key`, systemd unit, setup code. |
| `yanto self-update` | Download release binary, verify SHA256, DB backup, swap, restart (D19). |
| `yanto owner-reset` | Reset the Owner password and print a one-time reset link; deletes the Owner's sessions (replaces `npm run owner:reset`). |
| `yanto setup-code` | Issue a fresh one-time setup code while no Owner exists. |
| `yanto token create\|list\|revoke` | API Token management (CLI/dashboard only). Secret printed once. |
| `yanto mcp` | MCP over stdio (token via `YANTO_API_TOKEN_FILE`). |
| `yanto backup project <slug>` / `yanto backup yanto` / `backup list` / `backup prune` | Create and prune Backups; `backup project|yanto` is what scheduled Cron Entries call (D14). Exits non-zero on failure. |
| `yanto restore yanto <archive>` | Restore the Yanto DB Backup with the service stopped (04 §7.6). |
| `yanto deploy <slug> [--ref]` | Enqueue a Deployment locally; optional `--wait`/`--follow` (ASSUMPTION). |
| `yanto doctor` | Checks DB integrity, migrations, `app.key` presence/perms, Docker, Host Daemons, ports, disk. |
| `yanto db status` / `yanto db migrate` | Report or apply migrations without serving. |
| `yanto version` | Version, build, SQLite version. |

## Open questions / risks

1. API Tokens are not Project-scoped: a `write` token can deploy any Project. Project-scoped tokens would be a v1.x addition (DB: `api_token_projects`).
2. Deploy Token reversibility (doc 02, section 3) enables reveal + HMAC at the cost of a decryptable secret.
3. `container_exec` via REST (`admin`) is a root-equivalent remote shell for any admin token; ADR 0007 accepts root, but consider disabling REST exec by default (setting).
4. `deployment_cancel`, `backup_restore`, `cloudflare_tunnel_activate`, and per-session CSRF (instead of legacy rotation) are new or changed behavior; not in the parity checklist.
5. Session lifetime has no idle timeout (legacy parity); a shorter idle timeout may be preferable for an internet-reachable dashboard.
6. In-memory rate limits reset on restart; repeated restarts weaken brute-force protection.
7. Library behavior (`rmcp` tool API, tower/axum layers, template engine) is unverified; verify with Context7 before implementing.
