# Security Model & Project Direction

This doc captures the security model, known limitations, and the intended improvement direction — these reveal the project's constraints and where a reimplementation should do better.

---

## Security Model (what's in place)

### Authentication & session
- JWT HS256 in `yanto_session` httpOnly cookie (7-day expiry, issuer/audience checked).
- Session revocation via `session_version` bumping, checked at request auth time and every 15s during SSE streams.
- Password hashing: scrypt (N=16384, r=8, p=1, 16-byte salt, 64-byte key) with bcrypt legacy verify and opportunistic re-hashing.
- Rate limiting on login (5/15min), account setup (10/15min), `/deploy` (10/15min), GitHub webhook (60/min), worker register (10/15min), MCP auth failure (20/15min).

### Authorization
- Single owner (DB-enforced partial unique index) + invited members.
- Per-project permission grants (`deploy`, `runtime`, `config`, `secrets`, `backups`, `hostnames`).
- Container↔project mapping via compose labels, with folder-name uniqueness required for unambiguous authorization (409 otherwise).
- Information hiding: missing project access returns 404 (not 403) to hide existence.
- Stream authorization guard revalidates every 15s.

### Request hardening
- CSRF double-submit cookie with token rotation after each successful mutation.
- helmet with strict CSP (allows only Cloudflare Turnstile challenge scripts/frames).
- Zod validation on all route inputs.
- Raw-body capture for webhook signature verification.
- Shell-less subprocess spawning (no shell injection via args).
- Path-traversal guards on all filesystem path computation (slugify, normalize, containment checks).
- Container ID validation against `^[a-zA-Z0-9][a-zA-Z0-9_.-]*$` before use.
- Protected container names (`^yanto-(app|postgres)-\d+$`) cannot be controlled via the API.

### Secret management
- Secrets encrypted at rest with AES-256-GCM envelope (`enc:v1:iv:tag:data`) keyed by scrypt(jwtSecret, fixed salt).
- Encrypted: R2 access/secret keys, Turnstile secret, worker join token, Cloudflare API tokens, tunnel tokens, AI provider API keys.
- Public API exposes only presence booleans (`hasApiKey`, `hasApiToken`, `hasTurnstileSecretKey`), never the raw values.
- Tokens (deploy, worker, worker-join, MCP) are opaque prefixed strings; only a hash is stored; raw value returned once at creation.
- Constant-time comparison for tokens and CSRF.

### Audit
- Append-only audit log; every mutating endpoint (REST and MCP) records `{ actor, action, entityType, entityId?, projectId?, metadata? }`.
- MCP actions are attributed as `mcp:<tokenName>`.

### Bot protection
- Cloudflare Turnstile verification on setup/login endpoints, conditional on runtime-configured keys; verifies the returned `action` matches.

### Webhook verification
- GitHub webhook HMAC-SHA256 signature verified against the project's deploy token using the raw body, constant-time compared.

---

## Known Limitations (from the architecture review)

These are documented findings a reimplementation should address:

### Security
- **SEC-1:** Default admin credentials only warned, not enforced.
- **SEC-2:** `constantTimeEqual` has a length leak (pads to equal length but length itself is observable).
- **SEC-3:** (Now addressed) Rate limits on `/deploy` and GitHub webhook.
- **SEC-4:** Container ID passed to docker CLI — flag-injection risk (mitigated by regex validation, but the review flagged it).
- **SEC-5:** SSH `StrictHostKeyChecking=accept-new` (accepts new host keys without verification).
- **SEC-6:** Weak `isEncrypted` heuristic (validates structure but not cryptographically).
- **SEC-7:** (Now addressed) R2/Cloudflare secrets now encrypted at rest.
- **SEC-8:** Backup endpoint leaks server filesystem paths.
- **SEC-9:** Webhook rawBody fails on >1MB form-encoded bodies.
- **SEC-10:** (Now addressed) CSRF cookie now rotated after each mutation.

### Performance
- **PERF-1:** SSE log streaming previously polled DB every 700ms per client instead of using the event bus (now uses the bus).
- **PERF-2:** `docker stats --no-stream` on every container cache miss (expensive).
- **PERF-3:** `latestDeployments` fetches full `logs` column (~10MB/call).
- **PERF-4:** `appendDeploymentLog` does SELECT after UPDATE.
- **PERF-5:** `listProjectsWithContainerCounts` spawns `listContainers` on every dashboard load + 30s poll.
- **PERF-6:** Migrations run unconditionally every restart (no version tracking skip).
- **PERF-7:** (Now addressed) Worker polling now has exponential backoff.

### Architecture
- **ARCH-1:** In-memory `activeDeployments` Map doesn't survive restart/scale (only the master process owns it).
- **ARCH-2:** Deployments run in-process (OOM kills the API). Long-term goal: deployments in a child process.
- **ARCH-3:** Manual raw-SQL migrations, no version tracking (now uses Drizzle Kit migrations with a legacy bridge).
- **ARCH-4:** `pg.Pool` with default max 10, exhaustion risk under load.
- **ARCH-5:** (Now addressed) SIGINT handler now awaits in-flight deployments.

### Code quality
- 19 silent catch blocks (errors swallowed without logging).
- Masked-env save-back bug that could destroy secrets (preservation logic now in place).
- Duplicated utilities across services.
- Fragile rollback dedup logic.
- Unguarded `JSON.parse` in places.
- No DB-level size limit on `logs` (only app-side truncation).
- `listAuditLogs` no upper bound.
- `cleanupDocker` throws on partial failure.
- `runCommand` always resolves (never rejects — callers must check exitCode).

### Client
- `App.tsx` ~1781 lines / 44 useState (monolithic).
- Most handlers not `useCallback`, defeating `memo`.
- No code splitting.
- No data caching, blind 30s polling.
- Pagination duplicated 4x.
- SettingsView 26 props.
- Over-fetching on view switch (`loadAll()` refetches 10 resources after every mutation).

### Test coverage
- Zero route-handler tests, core services untested, auth middleware untested, migrations untested, zero React component tests, no E2E.
- Coverage thresholds are low (statements/lines 32, branches 70, functions 43).

---

## Improvement Direction (from plan/improvement docs)

### AI Task reliability & security hardening (7 phases)

Goal: harden the AI-task implementation without changing the public workflow (create task from Git project → run with API-key or signed-in Codex → stop/continue/review/commit/push/clean worktree).

**Compatibility rules:** keep statuses/provider protocols, keep task & Codex thread IDs valid, additive idempotent migrations, never auto-delete remote branches, never silently discard worktree changes, failed start must leave task usable, stop must mean the run actually stopped.

- **Phase 1:** Orchestration tests (concurrent starts, single capacity slot, provider-resolution failure, stop during prep, stop after spawn, runner callback throwing, graceful shutdown, startup orphan recovery, run finishing in SSE snapshot/subscribe gap, deleting running task/project) using injectable boundaries, no real credentials.
- **Phase 2:** Atomic run admission — partial unique index `agent_runs_one_running_per_task_idx WHERE status='running'`; resolve/validate model before mutating conversation; one DB transaction to lock task→running, insert run, insert follow-up; reserve in-memory capacity slot before async prep; translate unique-index conflict to existing `409`. Future multi-replica → Postgres advisory lock/lease table.
- **Phase 3:** Deterministic runner ownership & cleanup — single `ActiveAgentRun` handle; label containers `com.yanto.agent=true`, `...run-id`, `...task-id`; `signal.throwIfAborted()` before docker ops; prefer `docker create` + `docker start -a` over `docker run`; idempotent `finally` cleanup; await confirmed `docker rm -f` before resolving stop; on SIGINT/SIGTERM stop HTTP, abort runs, await cleanup with bounded timeout; at startup remove only `com.yanto.agent=true` containers whose runs are still running.
- **Phase 4:** Protect Codex account data — runner-image integration probe with fake credential/session sentinels under `CODEX_HOME`; verify model-executed shell cannot read/modify them; limit readable/writable task roots to `/workspace` + toolchain, exclude `/data/codex` from task commands; probe symlinks/subprocesses/scripts/Python/Node reads/network exfiltration; separate persistent account state from per-task session storage; default task-command network disabled; refuse Codex-account tasks when sandbox probe/profile unavailable.
- **Phase 5:** Race-free bounded SSE — subscribe before snapshot, buffer events, include run ID + sequence watermark in snapshot, flush buffered events newer than watermark, dedupe by `(runId, sequence)`, on EventSource error reload+reconnect only if still running, send only latest run's bounded recent activity, cap persisted tool payloads separately from command output.
- **Phase 6:** Lifecycle-aware deletion — project deletion rejects `409` if any task running (unless force); task deletion removes worktree + DB history but retains local & remote branches; `resumeExistingBranch=true` may reuse local branch only when matching remote exists and local can fast-forward.
- **Phase 7:** Precise status semantics — store agent-execution success separately from Git-automation success; on Git-automation failure keep assistant message + run result, set `review`, preserve worktree; retry terminal reconciliation on final-DB-write failure; associate follow-ups with the accepting run; clear `lastError` only after a new run is accepted.

**Phases 2/3/4 are release blockers** (concurrent worktree ownership, post-cancellation edits, credential exposure).

### Cloudflare Tunnel feature design

Goal: publish a deployed local service (e.g. `http://127.0.0.1:3000`) to a Cloudflare HTTPS hostname without the Cloudflare dashboard. Flow: `Cloudflare HTTPS edge → Cloudflare Tunnel → 127.0.0.1:<port>`; origin may stay HTTP.

- **Architecture:** one remotely-managed tunnel per deployment node; a single tunnel serves multiple projects via multiple ingress rules plus a final `http_status:404` catch-all. Master creates tunnel/config/DNS via Cloudflare API; the target node runs `cloudflared` using the tunnel token.
- **Cloudflare operations:** require API-token perms `Cloudflare Tunnel Edit` + `DNS Edit`; store accountId/zoneId/zoneName/token; create tunnel (`POST /accounts/{id}/cfd_tunnel`, name `yanto-{node_id}`, `config_src: cloudflare`); get token; push ingress config; upsert DNS CNAME proxied to `{tunnel_id}.cfargotunnel.com`; transactional local DB writes after successful Cloudflare calls.
- **Runtime:** Docker-managed `cloudflared` container per node, labeled `yanto.cloudflared.tunnel=<id>`, token in root-only env file, restarted on token/tunnel change. Worker-node follow-up: new worker job type `cloudflare_tunnel_sync` so workers start/restart local `cloudflared` and report connector status.
- **Edge cases:** multi-level subdomains may need advanced cert; tunnel configured while `cloudflared` stopped → Cloudflare `1016`; `127.0.0.1` is from the target node's perspective; moving a project between nodes must move its route; deleting a project disables its route and resyncs ingress; avoid auto-deleting DNS (may be shared).

### UI/API improvement plan

- Redesign project tiles (title/repo, vertical deployment/container status, tunnel summary, container count, grouped actions); tile copy actions for deploy URL, webhook URL, deploy token.
- Cloudflare: one hostname route per project; "Publish" becomes update when one exists; hide hostname fields behind a configure button; enforce one route per project via DB indexes.
- Deployment retry via the normal manual deploy endpoint (not replaying env overrides).
- Containers as a single vertical list per project group; remove backup-history status column; Postgres targets full-width before backup history; visible loading states across views/buttons.
- API: add `GET /api/projects/:id/deploy-token` returning `{ deployToken }`; keep public project list stripped of `deployToken`.

---

## Priority roadmap (from the architecture review)

1. **Phase 1 — critical security:** default admin creds, rate limits, helmet, CSRF rotation, secret encryption, deploy-token exposure, masked R2 keys, SSH host-key pinning.
2. **Phase 2 — perf quick wins:** SSE event bus, avoid `docker stats` on cache miss, avoid full `logs` column fetch, avoid `listContainers` on every dashboard load, migration version tracking.
3. **Phase 3 — architecture:** extract views, code split, shared Pagination, settings-local state, adopt Drizzle Kit migrations, log silent catches.
4. **Phase 4 — testing & DX:** route-handler tests, core service tests, auth middleware tests, migration tests, React component tests, E2E, pre-commit hooks, `.env.example` validation.
5. **Phase 5 — long-term:** deployments in a child process (not in-process), SWR/React Query for data caching, configurable SSH host-key checking, DB pool limits, re-enable `no-explicit-any`.

A Rust reimplementation can treat these as requirements rather than improvements — building them in from the start is cheaper than retrofitting.
