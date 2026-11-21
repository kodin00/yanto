# Authentication & Authorization

## Roles & Statuses

- **`UserRole`**: `"owner" | "member"`. **Exactly one owner is enforced** at the database layer (partial unique index on `role WHERE role = 'owner'`).
- **`UserStatus`**: `"invited" | "active" | "disabled"`.
- **`ProjectPermission`**: `"deploy" | "runtime" | "config" | "secrets" | "backups" | "tasks" | "hostnames"`.
- **`McpAccessLevel`**: `"read" | "write" | "admin"` (ranked: `read < write < admin`; a higher level grants all lower levels).

## Session Handling

- **Cookie name:** `yanto_session` (httpOnly, path `/`, sameSite `lax`, secure per runtime settings, maxAge **7 days**).
- **JWT** HS256, issuer `yanto`, audience `yanto-dashboard`, expiry 7d. Payload: `{ sub: userId, username, sessionVersion }`.
- `currentUser()` verifies the JWT (algorithms `["HS256"]`, audience+issuer checked) and returns a `SessionIdentity` **without a DB lookup**.
- `authenticateRequest()` then loads the full `AuthPrincipal` from the DB and rejects if the user is not `active`, or if `username`/`sessionVersion` changed since the token was issued.
- **Session revocation mechanism:** bumping `session_version` invalidates outstanding tokens. This is checked both at request auth time and periodically during streams.

## Auth Principal

`AuthPrincipal = { id, username, role, status, sessionVersion, projectAccess: UserProjectAccess[] }`. Attached to requests as the auth context.

## Middleware / Guards

- `requireAuth` — 401 if no principal.
- `requireOwner` — 403 if not owner (`"Owner access required."`).
- `requireProjectAccess(paramName)` — 404 if no access (information hiding — hides existence).
- `requireProjectPermission(permission, paramName)` — 404 if no access, 403 if missing the specific permission.
- **Owner bypasses all project access/permission checks** and sees all projects/containers.

### Container access (`projectForContainer`)

Maps a Docker container (by id or name) to a project via the Compose `com.docker.compose.project` label matching `projects.folder_name`. Requires the folder name to be unique across projects (else 404/409). Members can only act on containers whose compose project maps to exactly one project they can access. Ambiguous folder names block delegated access.

### Stream authorization guard

For SSE endpoints, revalidates the principal every **15s**; on revocation sends a terminal `{ kind: "authorization_revoked" }` / `{ error: "Authorization was revoked.", done: true }` event and closes the stream.

## Account Types & Views

Members get a base set of views (`dashboard`, `projects`, `deployments`, `containers`, `audit`) plus `backups`/`hostnames` if granted those project permissions. The full view enum: `dashboard | projects | tasks | deployments | containers | nodes | backups | hostnames | frp | dns | audit | settings`.

## Password Rules

- New passwords hashed with **scrypt** (16-byte random salt, 64-byte key, params N=16384, r=8, p=1), encoded as `scrypt$<N>$<r>$<p>$<salt base64url>$<hash base64url>`.
- **Legacy bcrypt** hashes (strings starting with `$2`) are accepted and verified via bcrypt compare.
- On next successful login, bcrypt hashes are opportunistically **re-hashed to scrypt**.
- Verification uses `timingSafeEqual`; never throws (returns false on any error).

## Owner Concept

Single owner (DB-enforced). Owner bypasses all project access/permission checks and sees all projects/containers. The `owner-reset` CLI is the recovery mechanism when the owner password is lost.

## Account Tokens (invite / reset)

Single-use, hashed tokens for `invite` (member invitation) and `reset` (password reset) flows:
- **24-hour TTL**, 32-byte base64url secret, stored as HMAC-SHA256 hash (keyed by JWT secret).
- Issuing a reset token **clears the user's password** and sets status to `invited`, bumping `sessionVersion` (invalidates sessions).
- Token URL format: `<appBaseUrl>/account/setup#token=<token>`.
- `completeAccountSetup` validates the token (unused, unexpired, user not disabled), marks all of the user's tokens used, sets password, flips status to `active`, bumps `sessionVersion`.
- `accountSetupDetails` previews the username from a valid token.

## CSRF Protection

- **Double-submit cookie** model: a `yanto_csrf` cookie (non-httpOnly, `SameSite=Lax`, `Secure` per runtime settings) plus an `x-csrf-token` header.
- For safe methods (GET/HEAD/OPTIONS): issues the cookie if missing.
- For mutating methods: validates the header matches the cookie via `timingSafeEqual`. On success, **rotates** the CSRF token (new cookie + `x-csrf-token` response header). On failure: 403 `"Invalid CSRF token."`.
- **CSRF-exempt paths:** `/deploy`, `/webhooks/github`, `/mcp`, `/api/workers/register`, and anything starting with `/api/workers/`.

## Rate Limiters

- **Login:** 5 per 15 min per IP (skip successful requests).
- **Account setup:** 10 per 15 min per IP (skip successful requests).
- **`/deploy` (manual deploy token):** 10 per 15 min.
- **GitHub webhook:** 60 per min.
- **Worker register:** 10 per 15 min.
- **MCP auth failure:** 20 per 15 min per IP (skip successful requests).

## Turnstile (Bot Protection)

- Enabled only when **both** site key and secret key are configured in runtime settings (not just env).
- `verifyTurnstileToken(req, token, action)`: no-op if disabled. Otherwise requires a token, POSTs to `https://challenges.cloudflare.com/turnstile/v0/siteverify` with `{ secret, response: token, remoteip, idempotency_key }` (10s timeout). `remoteip` from the `cf-connecting-ip` header.
- Throws 503 on network failure, 400 if the response isn't ok / `success` is false / the returned `action` doesn't match the expected action string.
- Action strings used: `owner_setup`, `login`, `account_setup`.

## SSE / Streaming Convention

- Headers: `Content-Type: text/event-stream`, `Cache-Control: no-cache, no-transform`, `Connection: keep-alive`, `X-Accel-Buffering: no`.
- Framing: `data: <JSON>\n\n`. Events are JSON objects, typically with a `done: boolean` field to signal termination.

## Response & Error Conventions

- Success bodies are route-specific.
- Error body shape is uniformly `{ message: string }`.
- Status codes: 400 (validation), 401 (auth required / invalid token), 403 (forbidden / insufficient permission), 404 (not found / hidden), 409 (conflict/state mismatch), 413 (payload too large — backups), 500 (internal; masked in production).
- Mutating endpoints consistently write audit log entries with `{ actor, action, entityType, entityId?, projectId?, metadata? }`.
