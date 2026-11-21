> Classification: INTERNAL

# 08 — Conventions for parallel milestones

Established by M1 (Identity) so that M2–M8 can be built in parallel worktrees without colliding. Where this doc and 01–03 disagree about *how code is organized*, this doc wins; behavior and data decisions stay in [00-scope](00-scope.md).

Quick map of the shared files every milestone touches. Each addition is **one line** (or one small block) so merges stay mechanical:

| You add | Central place | What to add |
|---|---|---|
| A migration | `crates/yanto-core/migrations/` | A new file in your range (section 1). Never edit another milestone's file. |
| A service | `crates/yanto-core/src/service/mod.rs` | `pub mod <area>;`, a field on `Services`, one line in `Services::new`, your specs in `all_operations()` |
| A port (host side effect) | `crates/yanto-core/src/ports/mod.rs` | `mod <port>;` and a `pub use` line (section 7) |
| Dashboard routes | `crates/yanto-server/src/dashboard/mod.rs` | `mod <area>;` and one `.merge(<area>::routes())` in the authenticated block |
| REST routes | `crates/yanto-server/src/api_v1/mod.rs` | `mod <area>;` and one `.merge(<area>::routes())` |
| A nav entry | `crates/yanto-server/src/nav.rs` | one `NavEntry` line in `NAV` |
| A CLI command | `crates/yanto-cli/src/{cli.rs,main.rs,cmd/mod.rs}` | a `Command` variant, a match arm, a `pub mod` |
| A permission-matrix row | your service's `#[cfg(test)]` matrix (section 5) | one grid row per operation |

## 1. Migrations

- Location: `crates/yanto-core/migrations/NNNN_<slug>.sql`, 4 digits, embedded by `sqlx::migrate!`. Forward-only; an applied file is immutable (sqlx checksums it). Fixes are new files.
- **Number ranges** (a milestone uses only its own range, so two worktrees never produce the same number):

| Milestone | Range | Notes |
|---|---|---|
| M0 | `0001`–`0099` | `0001_app_settings` |
| M1 | `0100`–`0199` | `0100` users, `0101` sessions, `0102` account_tokens, `0103` user_project_access/permissions, `0104` api_tokens, `0105` audit_logs |
| M2 | `0200`–`0299` | Projects first (`0200_projects`), then the FK rebuilds below |
| M3 | `0300`–`0399` | |
| M4 | `0400`–`0499` | |
| M5 | `0500`–`0599` | |
| M6 | `0600`–`0699` | |
| M7 | `0700`–`0799` | |
| M8 | `0800`–`0899` | |

  02 §2 mentions a single `0001_init.sql`; that is superseded by this per-milestone scheme.
- Tables are `STRICT`; timestamps are `INTEGER` unix ms; ids are `<prefix>_<24 hex>` (`crypto::new_id`). Prefixes in use: `usr`, `atk` (account token), `tok` (API Token), `aud`. Reserve new prefixes in the table of your own doc section.
- A migration that depends on another milestone's table must wait for that milestone to merge; do not create stubs for it.
- **Adding a foreign key to an M1 table (M2 does this).** SQLite cannot `ALTER TABLE ... ADD FOREIGN KEY`. `user_project_access` / `user_project_permissions` (0103) were created without the `projects` FK because `projects` does not exist yet, with final column names so the rebuild is a pure copy. M2's migration, after `0200_projects`, is:

```sql
-- inside the migration transaction
PRAGMA defer_foreign_keys = ON;
CREATE TABLE user_project_access_new (
    user_id    TEXT    NOT NULL REFERENCES users (id) ON DELETE CASCADE,
    project_id TEXT    NOT NULL REFERENCES projects (id) ON DELETE CASCADE,
    created_at INTEGER NOT NULL,
    updated_at INTEGER NOT NULL,
    PRIMARY KEY (user_id, project_id)
) STRICT;
-- Drop grants that point at Projects that never existed (M1 could not validate them).
INSERT INTO user_project_access_new
    SELECT a.* FROM user_project_access a WHERE a.project_id IN (SELECT id FROM projects);
-- Same for user_project_permissions_new (composite FK to the new access table).
DROP TABLE user_project_permissions;
DROP TABLE user_project_access;
ALTER TABLE user_project_access_new RENAME TO user_project_access;
-- recreate: user_project_access_project_idx, the user_project_access_no_owner trigger,
-- and user_project_permissions (with its CHECK and composite FK).
```

  Recreate every index and trigger of the rebuilt table (SQLite drops them with the table). For `audit_logs.project_id` do **not** rebuild the table: add `CREATE TRIGGER audit_project_detach AFTER DELETE ON projects BEGIN UPDATE audit_logs SET project_id = NULL WHERE project_id = OLD.id; END;` — the `audit_no_update` trigger from 0105 allows exactly that one mutation. Update the M1 service tests that insert grants for Projects that do not exist (they use ids such as `prj_1`; create the Project row first).
- Service-layer `user_access_update` in M2 must also reject unknown Project ids with 422 before the FK would fail.

## 2. Registering a feature module

A feature `foo` (for example M2 `projects`) touches the central places in this order:

1. **Service** (`yanto-core`): `service/foo.rs` with `pub const OP_FOO_BAR: OpSpec = ...` per operation and `pub struct FooService`. Then in `service/mod.rs`: `pub mod foo;`, a `pub foo: foo::FooService` field, `foo: foo::FooService::new(ctx.clone()),` in `Services::new`, and `v.extend(foo::OPERATIONS);` in `all_operations()`.
2. **Repository** (`db/repo/foo.rs`): SQL and row mapping only, functions take `&mut SqliteConnection`; add `pub mod foo;` in `db/repo/mod.rs`. Runtime-checked `sqlx::query*` (no `query!` macros, so no `.sqlx/` metadata; `sqlx::AssertSqlSafe` only for strings assembled from constants).
3. **Dashboard** (`yanto-server`): `dashboard/foo.rs` exposing `pub fn routes() -> Router<AppState>`; one `.merge(foo::routes())` line in the authenticated block of `dashboard::routes`. A route can only be public by being added to `auth::public_routes`; do not add public routes elsewhere.
4. **REST**: `api_v1/foo.rs` exposing `pub fn routes() -> Router<AppState>`; one `.merge(foo::routes())` in `api_v1::routes`. Handlers take `ApiActor` (Bearer only, cookies are ignored) and, for bodies, `ApiJson<T>`.
5. **Nav**: one `NavEntry { label, href, view: Some(AppView::Foo) }` in `nav.rs`. `view: None` shows the entry to every signed-in user; otherwise it shows when `Actor::app_views()` contains the view (derived from permissions, never stored).
6. **Templates**: under `templates/foo/` (section 3).
7. **Audit**: write one audit row per mutating operation through `service::audit::write(ctx, &mut tx, &AuditActor::from_actor(actor), &AuditEvent::new("foo.bar", "foo").entity(id).project(pid).meta(json!({...})))` **inside the same transaction** as the change. Action names are `<area>.<verb>` from 03 a.7. Metadata holds key names and counts, never secret values.
8. **Operation names** are the `noun_verb` names of 03 (`project_deploy`, ...); dashboard handler, REST route and the future MCP tool all call the service function of that name.

Handler shape (adapters stay thin):

```rust
async fn deploy(State(s): State<AppState>, session: Session, headers: HeaderMap, Path(id): Path<String>)
    -> Result<Response, DashError> {
    let out = s.services.foo.deploy(&session.0.actor, &id).await.map_err(|e| DashError::new(e, &headers))?;
    Ok(render(&Page { ctx: PageCtx::new(&session, "Title", "/foo"), out }))
}
```

Service shape (authorization first, then work, audit in the same transaction):

```rust
pub async fn deploy(&self, actor: &Actor, project: &str) -> Result<Deployment, CoreError> {
    actor.authorize(&OP_PROJECT_DEPLOY, Some(project))?;   // 404 not visible / 403 missing permission / 403 scope
    let mut tx = self.ctx.db.begin_write().await?;         // BEGIN IMMEDIATE
    /* repo calls, audit::write(..) */
    tx.commit().await.map_err(db_err)?;
    Ok(..)
}
```

Authorization rules live only in `OpSpec` constants: `OpSpec::new(name, Access::{Authenticated|Owner|Project(Need::View|Need::Permission(p))}, Scope::{Read|Write|Admin})`, plus `.session_only()` for dashboard/CLI-only operations (token issuance, sessions, exec). A token can never exceed its user: scope is checked in `authorize`, project grants come from the token's user. Operation scopes follow 03 a.5.

## 3. Templates and partials

- Pages live in `crates/yanto-server/templates/<area>/<page>.html` and extend `layout.html` (`{% extends "layout.html" %}`, fill `{% block content %}`; optional `{% block head %}` for page scripts).
- Every page struct has a `ctx: PageCtx` field (`PageCtx::new(&session, title, path)` signed in, `PageCtx::public(title)` otherwise). The layout reads `ctx.title`, `ctx.nav`, `ctx.username`, `ctx.csrf`.
- Fragments returned to htmx (`hx-target` swaps) live next to their page as `templates/<area>/_<fragment>.html` (leading underscore, no layout). Include shared fragments with `{% include "<area>/_fragment.html" %}`. Fragments that are three lines long are built with `escape()` in the handler instead (see `dashboard/users.rs`).
- Plain `<form method="post">` includes `<input type="hidden" name="_csrf" value="{{ ctx.csrf }}">`. htmx requests need nothing: `<body hx-headers>` sends `X-CSRF-Token` automatically. Use `hx-put/patch/delete` for non-POST verbs (HTML forms cannot).
- Pages that can render a secret (token, link) go through `render()`, which sets `Cache-Control: no-store`; show secrets exactly once and never put them in a redirect URL.
- No inline scripts or styles (CSP is `default-src 'self'`); scripts are files under `assets/js/` loaded with `defer`; vendored libraries are listed in `assets/vendor/README.md`.
- Pass pre-formatted strings to templates (`view::fmt_ts`, `fmt_opt_ts`); keep logic out of templates.

## 4. Errors to HTTP

`CoreError { kind: ErrorKind, message, source, retry_after_secs }` is the only error services return. Adapters never invent statuses; use `crate::error`:

| `ErrorKind` | HTTP | REST `error.code` | Notes |
|---|---|---|---|
| `Validation(fields)` | 422 | `validation_failed` | `error.fields[]` lists `{field, message}` |
| `Unauthenticated` | 401 | `unauthenticated` | REST adds `WWW-Authenticate: Bearer` |
| `Forbidden` | 403 | `forbidden` | missing permission/scope/Owner-only |
| `NotFound` | 404 | `not_found` | also "not visible to you" (hides existence) |
| `Conflict` | 409 | `conflict` | duplicates, referenced rows |
| `RateLimited` | 429 | `rate_limited` | `Retry-After` from `retry_after_secs` |
| `Unavailable` | 503 | `unavailable` | Docker/daemon down |
| `Internal` | 500 | `internal` | message replaced by `internal error`; details only in logs |

REST body: `{"error":{"code","message","fields"?}}`. Malformed JSON, bad query strings: 400 `bad_request` (never reaches a service). Unknown `/api/v1` path: 404 `not_found`.

Dashboard: handlers return `Result<Response, DashError>`. Full-page requests get `error.html` with the mapped status; htmx requests (`HX-Request: true`) get a `<div class="toast-msg error">` fragment with `HX-Retarget: #toast` and `HX-Reswap: innerHTML` (the layout configures htmx to swap 4xx/5xx responses). Forms that can fail validation re-render themselves with the message and the mapped status (see `dashboard/auth.rs`) instead of returning `DashError`. Unauthenticated: 303 to `/login` (or `/setup` while no Owner exists); htmx gets 401 + `HX-Redirect`.

## 5. Permission-matrix tests

Every operation must have a row in a grid and an invocation in the matrix test; a new operation without a row fails the build.

- Core matrix: `crates/yanto-core/src/service/tests/matrix.rs`. `GRID` is a hand-written table (`Y`/`N` per actor profile: owner session, member session, owner/member tokens at read/write/admin, CLI). `grid_and_operation_registry_match_exactly` compares it to `service::all_operations()`. `every_actor_profile_against_every_operation` checks both `authorize()` and the real service fn (a missing `authorize` call inside a service fails here).
- Adapter matrix: `crates/yanto-server/tests/matrix.rs` repeats it over HTTP for REST routes (principals x scopes, anonymous 401, cookie-only 401) and dashboard routes (anonymous redirect, Owner, Member 403, bearer-only is anonymous).
- **Adding rows for a Project-scoped feature (M2+):** extend `PROFILES` with Members that carry different grants, and add rows per permission, e.g. for `project_deploy` (`Permission::Deploy`, `Scope::Write`) the expectations are:

  | actor | expected |
  |---|---|
  | Owner session / Owner token write+admin / CLI | allowed |
  | Owner token read | 403 (scope) |
  | Member with `deploy` on the project, session | allowed |
  | same Member, token read | 403 (scope) |
  | same Member, token write/admin | allowed (scope never widens the user) |
  | Member with access but without `deploy` | 403 |
  | Member with no access row for the project | **404** (existence hidden) |
  | Member's token on a project of another Member | 404 |

  Build such Members with `core.create_member("m", &[("prj_x", &[Permission::Deploy])])` and call `actor.authorize(&OP, Some("prj_x"))` plus the service fn. Test both the "allowed" and "denied" cell for each permission so a swapped permission is caught.
- Always include the cross-checks: a token can never reach a `.session_only()` operation; Owner-only operations are 403 for Members even with an `admin` token; unknown ids behave the same for authorized and unauthorized callers where 404 is specified.

## 6. Test helpers

Enabled by the `testing` feature (always on for a crate's own tests; the `yanto-server` crate lists itself as a dev-dependency with the feature). Use them instead of building apps by hand.

`yanto_core::testing`:

- `TestCore::new().await`: temp-dir SQLite, `FakeClock` (starts 2026-01-01; `core.clock.advance(Duration)`), cheap Argon2 parameters, real `Services`.
- `core.setup_owner()`, `core.create_member(name, &[(project, &[Permission..])])` (goes through the real invite flow, so the user is `active`), returning `TestUser { id, username, password }`.
- `core.session_actor(&user)`: a dashboard `Actor` built from a real login. `core.token_actor(&user, Scope)`: `(bearer, Actor)` from a real token. `Actor::cli()` is the CLI actor.
- `testing::token_from_link(url)`: pulls the token out of an invite/reset link.

`yanto_server::testing`:

- `TestApp::new().await` (or `with_public_url`). `app.client()` anonymous; `app.owner_client()`, `app.member_client(name, grants)` sign in through `POST /login` and read the CSRF token from the rendered page; `app.bearer(&user, Scope)` returns a Bearer token.
- `TestClient`: `get`, `post_form` (adds `_csrf` when signed in), `form(method, ..)`, `htmx(method, path, fields)` (sets `HX-Request`), `json(method, path, &Value)`, `send(..)`; builders `with_bearer`, `with_ip`, `with_origin(Option)`, `without_csrf`. Requests carry `Host`/`Origin` and a fake peer address so rate limits and audit IPs are testable.
- `TestResponse`: `status`, `text()`, `json::<T>()`, `value()`, `header(name)`, `location()`, `session_cookie()`, `set_cookie_header()`, `csrf_token()`.

CLI end-to-end tests run the real binary (`env!("CARGO_BIN_EXE_yanto")`) against a temp `data_dir` (see `crates/yanto-cli/tests/identity.rs`).

Always-on checks: `cargo fmt --check`, `cargo clippy --workspace --all-targets --locked -- -D warnings`, `cargo test --workspace --locked`. Tests never touch Docker, `/etc` or the network.

## 7. Ports and fakes for host side effects

Anything with a side effect outside SQLite and the data dir (time, Docker, Compose, git, `/etc/cron.d`, `authorized_keys`, systemd, Traefik, `frps`, Cloudflare, registry) goes behind a port:

1. **Trait in core**: `crates/yanto-core/src/ports/<concern>.rs` (`mod <concern>;` plus a `pub use` in `ports/mod.rs`). Object-safe, `Send + Sync + 'static`, small and intention-revealing (`CronPort::write_entries(&[CronEntry])`, not `write_file`). Async ports use `#[async_trait::async_trait]` (add the crate when the first async port lands). Domain types used in the signature live in core.
2. **Real implementation in `yanto-host`**: `crates/yanto-host/src/<concern>.rs`, one module per concern; it may use `std::fs`, `tokio::process`, `bollard`, `reqwest`. File writers go through one atomic helper (temp file, fsync, rename, `flock`, refuse symlinks).
3. **Fake in core's `testing` module**: `crates/yanto-core/src/testing/<concern>.rs` (`pub mod <concern>;` in `testing/mod.rs`). A fake records calls in a `Mutex<Vec<_>>` and returns scripted results (`FakeCron::fail_next(err)`), so tests can assert "exactly these entries were written" and error paths. `FakeClock` is the reference example.
4. **Inject through `ServiceDeps`**: add a field (`pub cron: Arc<dyn CronPort>`), store it on `Ctx` (or on the one service that needs it), and wire the real implementation in the composition root `crates/yanto-cli/src/cmd/common.rs::open`. `TestCore::new` wires the fake and exposes it as a field (`core.cron`) so tests can inspect it.
5. **Services decide, ports do**: validation, authorization and audit are in the service; the port performs one effect. A service never calls `std::fs` outside `data_dir`, never spawns a process, never reads the system clock (use `ctx.now()`).
6. **Host-integration tests** (fixture `/etc` trees, real Docker) live in `yanto-host/tests/` and are the only tests allowed to touch the filesystem outside a tempdir; Docker-dependent tests are marked `#[ignore]` and run in the dedicated CI job.

## 8. Security-relevant conventions

- Secrets: wrap in `secrets::Secret` (redacting `Debug`/`Display`); store `*_enc` columns with `Secrets::seal(table, column, row_id, &secret)` / `open` (AAD binds the location); tokens and codes are stored as SHA-256 hex (`crypto::sha256_hex`) and compared with `crypto::ct_eq`. Never log a token, code, cookie, password or link; audit metadata never holds one.
- Cookie auth only on the dashboard (`dashboard::session`), Bearer only on `/api/v1` (`ApiActor`). The CSRF layer covers every unsafe dashboard request; the Origin check also covers public POSTs.
- Rate limits live in `service::Limits` (in-memory, per process, `ratelimit::FailureLimiter`); client IPs come from `net::resolve_origin` (forwarded headers only from `runtime.trusted_proxies`).
- Every mutating operation writes an audit row in the same transaction; failures of security events (login failure, rate limit) are written too, with `outcome = denied`.

## 9. M1 deviations from 00–07 (recorded for later readers)

- `user_project_access` / `user_project_permissions` / `audit_logs.project_id` have no `projects` FK until M2 (section 1). 02 specifies the FKs.
- API Token name uniqueness is per user among non-revoked tokens (02: global), so a Member cannot probe other users' token names.
- Sessions are implemented in the service layer (`AuthService`, table `sessions` keyed by SHA-256 of the cookie) with a small axum middleware, not with the `tower-sessions` crate (01 §5.2). Same schema and behavior; the CLI and tests can authenticate sessions without the HTTP stack.
- Session lifetime is 24 h idle and 7 days absolute (D30 wins over 03 a.3 "7 days, no idle timeout"). `last_seen_at` is touched at most once a minute, so idle detection has a one-minute granularity.
- The setup code is stored as two `app_settings` keys: `setup.code_hash` (SHA-256 hex JSON string) and `setup.code_expires_at` (unix ms). Format `XXXX-XXXX-XXXX-XXXX`, 80 bits, 24 h, re-issuable with `yanto setup-code` while no Owner exists (OQ-4 recommendation). A second setup attempt after the Owner exists fails with 401 (the code is gone), not 409.
- `AppView::Users` was added for the Owner-only Users page (03 a.2 lists no view for it). Account pages (`/settings/api-tokens`, `/settings/sessions`) have `view: None` so Members see them.
- Extra audit actions beyond 03 a.7: `auth.password_change`, `auth.setup_code_issue`, `auth.setup_failed`.
- `GET /api/v1/users/{id}` is exposed in addition to the 03 list (it is the `user_get` operation behind the dashboard detail page).
- `Retry-After` for 429 is carried in a new `CoreError::retry_after_secs` field (the `ErrorKind` enum is unchanged).
- The SQLite file is created `0600` (02 §1 says so; M0 left it at the umask default).
- Rate limits are per process (OQ-7 recommendation); `runtime.trusted_proxies` is read from `app_settings` at startup only.
- Queries are runtime-checked (`sqlx::query`), as in M0, so there is no `.sqlx/` directory and `sqlx prepare --check` has nothing to check until macros are adopted.
