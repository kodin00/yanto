# Services (Miscellaneous)

This doc covers the cross-cutting services not detailed in the other functional docs: accounts, audit, backups, crypto, passwords, settings, system, tokens, turnstile, and GitHub webhooks.

---

## accounts.ts — User identity, auth, RBAC, lifecycle

**Responsibility:** User identity, authentication, RBAC, account lifecycle (owner + invited members), project-access grants, and account-setup token flow.

- **Roles:** `owner` (sees all views) and `member`. **Statuses:** `invited`, `active`, `disabled`.
- **Views / permissions:** 12 app views. Members get a base set (`dashboard`, `projects`, `deployments`, `containers`, `audit`) plus `backups`/`hostnames` if granted those project permissions. Project permissions are a fixed set: `deploy`, `runtime`, `config`, `secrets`, `backups`, `hostnames`.
- **Auth:** `authenticateUser` does case-insensitive username lookup, verifies password, rejects non-`active` users, and lazily re-hashes the password if the stored hash needs upgrading; updates `lastLoginAt`. `loadPrincipalByUserId` loads the user + their per-project permission grants.
- **Setup:** `setupStatus` reports `needsSetup` when zero users exist. `createInitialOwner` is gated by an optional `YANTO_SETUP_CODE` (constant-time compared) and uses a Postgres advisory lock + transaction to ensure exactly one owner.
- **Account tokens (invite/reset):** 24h TTL, 32-byte base64url secret, stored as HMAC-SHA256 hash (keyed by JWT secret). Issuing a reset token clears the user's password and sets status to `invited`, bumping `sessionVersion` (invalidates sessions). Token URL format: `<appBaseUrl>/account/setup#token=<token>`. `completeAccountSetup` validates the token (unused, unexpired, user not disabled), marks all of the user's tokens used, sets password, flips status to `active`, bumps `sessionVersion`. `accountSetupDetails` previews the username from a valid token.
- **Member management:** `createMember` (invited, no password, gets invite token + setup URL), `replaceProjectAccess` (owner cannot be edited this way; each project assignable once), `setUserStatus` (owner cannot be disabled; cannot enable a user with no password), `deleteMember` (owner cannot be removed), `createResetLink`, `recoverOwnerAccount` (issues reset link for the owner), `listManagedUsers`.

---

## audit.ts — Append-only audit log

**Responsibility:** Append-only audit log writer and paged/project-scoped reader.

- **Audit log shape (per record):** `id` (`aud_…`), `actor` (defaults to `"admin"`, or `"github"`/`"worker"`/`"mcp:<tokenName>"`), `action` (string), `entityType`, `entityId` (nullable), `projectId` (nullable), `metadata` (free-form JSON object), `createdAt`.
- **Behaviors:** `recordAuditLog` never throws (swallows insert errors). `listAuditLogs` (capped at 500), `listAuditLogsForProjects` (filtered by a set of project IDs), and `paginatedAuditLogs` (page/pageSize with total count; can filter by single `projectId` or a project-ID whitelist, returning `false` when the whitelist is empty).

---

## backups.ts — Postgres database backups

**Responsibility:** Postgres database backups — dump the Yanto app DB or Postgres containers, restore into containers, upload to R2, delete, track downloads.

- **Backup record shape:** `id` (`bak_…`), `projectId` (nullable), `kind` (`"postgres"` for the app DB or `"postgres-container"` for a container), `status` (`running`/`success`/`failed`), `filename`, `filePath`, `fileSizeBytes`, `error`, `note`, `createdAt`, `finishedAt`, `downloadedAt`, `downloadCount`.
- **Backup format:** gzip-compressed SQL (`.sql.gz`) at max compression (level 9). Filenames are slugified `<owner>-postgres-<timestamp>.sql.gz`. Restore detects custom-format dumps (`.dump`/`.backup` extension or `PGDMP` magic bytes) and uses `pg_restore --clean --if-exists --no-owner`; otherwise pipes through `psql -v ON_ERROR_STOP=1` after dropping+recreating the `public` schema.
- **Dump sources:** App DB via `pg_dump <databaseUrl>`; container DBs via `docker exec <id> sh -lc '… pg_dump …'` reading `POSTGRES_USER`/`POSTGRES_DB`/`POSTGRES_PASSWORD` env from the container. Container targets must be `running`.
- **Target discovery:** `listPostgresBackupTargets` enumerates Docker containers flagged as Postgres candidates, inspects their env for DB name/user, and maps the container's compose project to a Yanto project by `folderName`.
- **Destinations:** Local filesystem (`config.backupsDir`, mode 0700 dirs, 0600 files). **R2 upload** is a separate explicit step (`uploadBackupToR2`) using the `r2` service; content type is `application/gzip` for `.gz`.
- **Lifecycle:** `createPostgresBackup` records a `running` row, dumps, then marks `success` (with size) or `failed` (with error message, deleting the partial file). `deleteBackup` refuses to delete while `running`. `markBackupDownloaded` increments `downloadCount` and sets `downloadedAt`.

---

## crypto.ts — Symmetric envelope encryption

**Responsibility:** Symmetric envelope encryption for stored secrets (R2 keys, Turnstile secret, worker join token, Cloudflare API tokens, tunnel tokens, AI provider API keys).

- **Scheme:** AES-256-GCM, 16-byte IV, 16-byte auth tag. Key derived once via `scrypt(jwtSecret, "yanto-tunnel-token-salt", 32)` (cached).
- **Envelope format:** `enc:v1:<iv b64>:<tag b64>:<ciphertext b64>`. Also tolerates a legacy 3-part `iv:tag:data` form.
- **Helpers:** `encrypt`, `decrypt` (throws on malformed), `isEncrypted` (validates structure + base64 + expected IV/tag lengths without throwing). Decoding rejects non-canonical base64 and mismatched lengths.

---

## passwords.ts — Password hashing and verification

**Responsibility:** Password hashing and verification with two supported algorithms.

- **Hashing (new):** scrypt, 16-byte random salt, 64-byte key, params N=16384, r=8, p=1. Encoded as `scrypt$<N>$<r>$<p>$<salt base64url>$<hash base64url>`.
- **Verification:** Accepts legacy bcrypt hashes (any string starting with `$2`, verified via bcrypt compare) and scrypt hashes. Validates scrypt params are integers within sane bounds; uses `timingSafeEqual` on the derived key. Never throws (returns false on any error).
- **Upgrade detection:** `passwordHashNeedsUpgrade` returns true for anything not starting with `scrypt$` (i.e. bcrypt hashes get opportunistically re-hashed to scrypt on next successful login).

---

## settings.ts — Persistent application settings

**Responsibility:** Persistent application settings stored as JSON rows in an `appSettings` key/value table, with at-rest encryption for secrets and an in-memory cached runtime-settings singleton.

### Settings groups

- **Runtime settings** (`runtime.settings`): `appBaseUrl` (trailing slash stripped), `cookieSecure` (bool), `turnstileSiteKey`, `turnstileSecretKey` (encrypted at rest; `hasTurnstileSecretKey` bool exposed publicly), `commandTimeoutMs`, `commandOutputMaxBytes`, `deploymentLogMaxChars`. Defaults come from `config`. The secret key can be cleared via `clearTurnstileSecretKey`. Public view omits the raw secret.
- **R2 settings** (`cloudflare.r2`): `enabled`, `accountId`, `bucket`, `accessKeyId`, `secretAccessKey` (both encrypted at rest), `prefix` (default `"postgres-dumps"`). Public view masks the access key (`****<last4>`) and only reports presence booleans. Saving preserves existing credentials when the submitted field is blank or equals the masked form.
- **Setup wizard** (`setup.wizard`): `completedAt`, `dismissedAt`, `updatedAt` timestamps. `saveSetupWizard("completed"|"dismissed")` sets the corresponding timestamp (transactional).
- **Multi-node feature** (`feature.multi_node`): `enabled` bool; public view adds a hardcoded `releaseStage: "beta"`.
- **Worker join token** (`worker.join_token`): stored encrypted; `ensureWorkerJoinToken` generates one (`ywj_…`) if none exists (env override `config.workerJoinToken` takes priority).
- **Cloudflare settings** (`cloudflare.tunnel`): `{ accountId, zoneId, apiToken }` (token encrypted at rest; only `hasApiToken` exposed publicly).
- **FRP public host** (`frp.public_host`): the public IP or DNS-only hostname of the VPS.

### Secret read helper

`readStoredSecret` decrypts encrypted values, returns `""` on decrypt failure. `normalizeString` trims non-strings to `""`.

---

## system.ts — Health checks and host resource usage

**Responsibility:** Health checks and host resource usage reporting.

- **`healthStatus`:** Returns `{ ok, uptimeSeconds, checkedAt, checks: { database, docker } }`. Each check is `{ ok: bool, message? }`. Database check runs `SELECT 1` with a 3s timeout; Docker check runs `docker version --format {{.Server.Version}}` with 3s timeout + process-group kill on timeout. Overall `ok` is the AND of all checks.
- **`systemUsage`:** CPU load percent (1-min load / CPU count, capped 100), memory total/used/free/usedPercent (from OS), and per-mount storage via `df -kP / /projects` (filesystem, size, used, available, usedPercent, mount).

---

## tokens.ts — ID and token generation

**Responsibility:** ID and token generation plus token hashing/comparison helpers.

- **IDs:** `createId(prefix)` → `<prefix>_<24 hex chars>` (12 random bytes).
- **Token types (all 32 random bytes, base64url):**
  - Deploy token: `ydp_…`
  - Worker token: `ywk_…`
  - Worker join token: `ywj_…`
  - MCP token: `ymcp_…`
- **`hashToken(token, secret?)`:** HMAC-SHA256 with `secret` if provided, else plain SHA-256. Used for account-token hashing (keyed by JWT secret), worker token hashing (keyed by worker token secret), and MCP token hashing (keyed by MCP token secret).
- **`constantTimeEqual(a, b)`:** constant-time string comparison (pads to equal length, checks both content and length equality).

---

## turnstile.ts — Cloudflare Turnstile (bot protection)

**Responsibility:** Cloudflare Turnstile verification gating auth flows.

- **`turnstileEnabled`:** true only when both site key and secret key are configured in runtime settings.
- **`verifyTurnstileToken(req, token, action)`:** No-op if Turnstile disabled. Otherwise requires a token, POSTs to `https://challenges.cloudflare.com/turnstile/v0/siteverify` with `{ secret, response: token, remoteip, idempotency_key }` (10s timeout). `remoteip` is taken from the `cf-connecting-ip` header (set by Cloudflare at the edge; omitted when absent — app is assumed behind a reverse proxy). Throws 503 on network failure, 400 if the response isn't ok / `success` is false / the returned `action` doesn't match the expected action string.

---

## github-webhooks.ts — GitHub webhook parsing & verification

**Responsibility:** GitHub webhook payload parsing, signature verification, and branch matching.

- **Payload schema:** `ref`, `deleted`, `after`, and `repository.{ name, full_name, default_branch, clone_url, ssh_url }` — all optional.
- **Signature:** `githubSignature(secret, rawBody)` = `sha256=<HMAC-SHA256(secret, rawBody)>`. `verifyGithubSignature` requires a `sha256=`-prefixed signature and constant-time-compares it against the computed signature over the raw body.
- **Branch logic:** `githubBranchFromRef` extracts the branch from `refs/heads/<branch>` (returns null for non-branch refs). `projectDeployBranch` returns the project's configured branch or `"master"` if blank.
- **Body extraction:** `githubPayloadFromRequestBody` unwraps a form-encoded `payload` field (JSON string) if present (GitHub's form-encoded webhook delivery), otherwise returns the body as-is.

---

## utils.ts

Single helper `normalizeString(value)` — returns trimmed string or `""` for non-strings. Shared by settings/services.

---

## Cross-cutting notes for reimplementation

- **Security primitives to replicate:** scrypt password hashing (N=16384,r=8,p=1, 16-byte salt, 64-byte key, `scrypt$…` encoding) with bcrypt legacy verify; AES-256-GCM envelope `enc:v1:iv:tag:data` keyed by scrypt(jwtSecret, fixed salt); HMAC-SHA256 token hashing; constant-time comparisons; CSRF token cookie round-trip; session-version invalidation on status/password/access changes.
- **Two distinct token families:** opaque prefixed tokens (`ydp_`/`ywk_`/`ywj_`/`ymcp_`, 32 random bytes base64url) and 24h account-setup tokens (32-byte base64url, HMAC-hashed at rest).
- **Streaming:** deployment logs, container logs, and agent task events use SSE streams — the reimplementation needs equivalent streaming endpoints.
- **Bot protection:** Turnstile verification is conditional on having both site+secret keys configured at runtime (not just env), and verifies the returned `action` matches the expected one.
- **Multi-tenancy model:** single owner + invited members with per-project permission grants; views are derived from role + granted permissions.
- **Audit logging:** every mutating endpoint (REST and MCP) writes an audit log entry; the writer never throws (swallows insert errors) so a logging failure never breaks the operation.
