# Yanto: Comprehensive Codebase Review

**Date:** 2026-05-30  
**Scope:** Full project — server, client, tests, config, security, architecture  
**Methodology:** 3 parallel audit angles (server architecture, client React perf, test/DX coverage)

---

## Table of Contents

1. [Security (10 findings)](#security)
2. [Performance (7 findings)](#performance)
3. [Architecture (5 findings)](#architecture)
4. [React / Client (14 findings)](#react--client)
5. [Code Quality (13 findings)](#code-quality)
6. [Test Coverage (7 findings)](#test-coverage)
7. [Developer Experience (10 findings)](#developer-experience)
8. [Priority Roadmap](#priority-roadmap)

---

## Security

### SEC-1: Default credentials with only warnings, no enforcement
**File:** `src/server/config.ts:3,9,18,20`  
Default `adminPassword` is `"admin"`, default `jwtSecret` is `"change-this-to-a-long-random-secret"`. `warnOnUnsafeDefaults()` only logs in production — does not refuse to start.  
**Impact:** Deploy without env vars → trivially guessable admin access.

### SEC-2: `constantTimeEqual` leaks length via early return
**File:** `src/server/services/tokens.ts:23-29`  
When `left.length !== right.length`, returns `false` immediately instead of constant-time comparison. Leaks token length via timing.  
**Impact:** Attacker measures response times to determine token length, enabling targeted brute-force.

### SEC-3: No rate limiting on `/deploy` or `/webhooks/github`
**File:** `src/server/routes/workers.ts:108-127,129-204`  
Login has rate limiting (5/15min), but deploy token and webhook endpoints have none.  
**Impact:** Brute-force deploy tokens at full speed.

### SEC-4: Container ID passed unsanitized to `docker` CLI
**File:** `src/server/services/docker.ts:118-124,137-153`  
`containerId` from `req.params.id` passed directly as docker argument. While `shell: false` prevents shell injection, a crafted ID like `--format={{.Config.Env}}` could be interpreted as a Docker CLI flag.  
**Impact:** Dump container environment variables (secrets).

### SEC-5: SSH StrictHostKeyChecking disabled
**File:** `src/server/services/ssh.ts:35`  
`GIT_SSH_COMMAND` includes `StrictHostKeyChecking=accept-new`, auto-accepting any host key.  
**Impact:** MITM attack on Git remote.

### SEC-6: `isEncrypted` heuristic is weak
**File:** `src/server/services/crypto.ts:38-41`  
Check `parts.length === 3 && parts.every(p => /^[A-Za-z0-9+/=]+$/.test(p))` matches many base64 strings that aren't encrypted.  
**Impact:** Pre-encryption tunnel tokens crash `decrypt()`.

### SEC-7: R2/Cloudflare secrets stored plaintext in database
**File:** `src/server/services/settings.ts:87-108`, `src/server/services/cloudflare.ts:95-112`  
Only tunnel tokens are encrypted. R2 `secretAccessKey` and Cloudflare `apiToken` are plaintext JSON in `app_settings`.  
**Impact:** DB dump exposes cloud credentials.

### SEC-8: Backup endpoint exposes internal file paths
**File:** `src/server/routes/backups.ts:123-136`  
`listBackups` returns full server filesystem paths like `/data/backups/somefile.sql.gz`.  
**Impact:** XSS or API abuse reveals filesystem layout.

### SEC-9: GitHub webhook rawBody capture fails on large/form-encoded payloads
**File:** `src/server/index.ts:28-35`, `src/server/routes/workers.ts:139`  
If payload exceeds 1MB JSON limit or is `application/x-www-form-urlencoded`, `rawBody` is undefined → signature verification silently rejects legitimate webhooks.

### SEC-10: CSRF cookie never rotated
**File:** `src/server/index.ts:52-62`  
Once set, CSRF cookie persists with same value for session lifetime. No rotation mechanism.  
**Impact:** Leaked CSRF token remains valid indefinitely.

---

## Performance

### PERF-1: SSE log streaming polls DB every 700ms per client
**File:** `src/server/routes/deployments.ts:42-100`  
Each SSE client creates `setInterval` polling `findDeployment(id)`. 50 viewers = 71 queries/sec. Already has `deploymentEvents` EventEmitter but SSE doesn't use it.  
**Fix:** Wire SSE to `deploymentEvents` bus, push on log append instead of polling.

### PERF-2: `listContainers()` runs `docker stats --no-stream` on every cache miss
**File:** `src/server/services/docker.ts:75-116`  
`docker stats` takes 2-5s. Cache invalidated after every stop/restart → next request re-fetches from scratch.  
**Fix:** Keep cache warm after invalidation; don't invalidate on stop/restart, just refresh in background.

### PERF-3: `latestDeployments()` fetches full `logs` column
**File:** `src/server/services/deployments.ts:149-173`  
Selects `logs` (TEXT, up to 500KB) for all 20 deployments. ~10MB per API call.  
**Fix:** Exclude `logs` from list queries. Only fetch on individual deployment detail.

### PERF-4: `appendDeploymentLog` does SELECT after every UPDATE
**File:** `src/server/services/deployments.ts:16-39`  
After every UPDATE, a separate `findDeployment` query doubles round-trips during deployments.  
**Fix:** Return updated logs from the UPDATE query directly, or use `RETURNING` clause.

### PERF-5: `listProjectsWithContainerCounts` fetches all containers on every call
**File:** `src/server/services/projects.ts:34-55`  
Runs `listContainers()` (docker CLI spawn) every time, filters in JS. Hit by `/api/projects` on every dashboard load and 30s poll.  
**Fix:** Cache project-container mapping with short TTL; or compute counts client-side from already-fetched container list.

### PERF-6: Migration runs every startup with unconditional UPDATE
**File:** `src/server/db/index.ts:12-194`  
`ALTER TABLE ... DROP NOT NULL` and UPDATE statements run unconditionally on every restart.  
**Fix:** Track migration version, skip already-applied migrations.

### PERF-7: Worker polling has no backoff on errors
**File:** `src/server/worker.ts:143-155`  
If master is down, worker hammers it with failed requests every 3s indefinitely.  
**Fix:** Exponential backoff on consecutive failures.

---

## Architecture

### ARCH-1: `activeDeployments` Map doesn't survive restarts or scale horizontally
**File:** `src/server/services/deployments.ts:10`  
In-memory Map for tracking running deployments. Empty after restart, defeats deduplication in multi-instance setups.

### ARCH-2: Deployment runs in same process as HTTP server
**File:** `src/server/services/deployments.ts:141`  
`void runLocalDeployment(...)` — fire-and-forget in main process. OOM in deployment crashes the API server.  
**Fix:** Run deployments in a child process or worker thread.

### ARCH-3: Manual migration instead of framework
**File:** `src/server/db/index.ts:12-194`  
~40 raw SQL strings with no version tracking, rollback, or transactional safety. Drizzle schema file exists but unused for migrations.  
**Fix:** Adopt Drizzle Kit migrations properly.

### ARCH-4: DB pool with no connection limits
**File:** `src/server/db/index.ts:6-8`  
`new pg.Pool({ connectionString })` uses defaults (10 max connections). Under load with concurrent deployments + SSE streams, pool exhaustion is likely.

### ARCH-5: SIGINT handler doesn't wait for in-flight deployments
**File:** `src/server/index.ts:130-133`  
`pool.end()` then `process.exit(0)` — in-flight log appends/status updates fail. Partial logs lost until next restart recovery.

---

## React / Client

### CLIENT-1: App.tsx is 1781 lines with 44 useState hooks
**File:** `src/client/App.tsx:175-1698`  
Every state change re-renders the entire component tree. No boundaries.  
**Fix:** Extract state into view-level components or useReducer + Context.

### CLIENT-2: 25+ handlers not wrapped in useCallback — defeats all memo() wrappers
**File:** `src/client/App.tsx:509-1038`  
Only 7 of 32+ handlers use `useCallback`. New function references on every render defeat `memo()` on all 8 view components.  
**Fix:** Wrap all handlers in `useCallback` with proper dependency arrays.

### CLIENT-3: No code splitting — all 8 views eagerly imported
**File:** `src/client/App.tsx:26`  
No `React.lazy()` or dynamic `import()`. Users download all view code on initial load.  
**Fix:** `React.lazy(() => import("./views/SettingsView"))` etc.

### CLIENT-4: `backdrop-filter: blur(14px)` on `.panel` class
**File:** `src/client/styles.css:670`  
Applied to every panel across the app. Forces GPU compositing layer per panel. On integrated GPUs, visible frame drops during scroll.  
**Fix:** Remove from `.panel`, keep only on `.sidebar` and `.login-panel`.

### CLIENT-5: No data caching in api.ts; 30-second blind polling
**File:** `src/client/lib/api.ts:45-71`, `src/client/App.tsx:361-367`  
Zero caching, no deduplication, no stale-while-revalidate. Every call hits network. 30s poll refetches everything unconditionally.  
**Fix:** Add SWR/React Query or basic request dedup + ETag support.

### CLIENT-6: Pagination component duplicated in 4 files
**File:** `src/client/views/ProjectsView.tsx:161-196`, `DeploymentsView.tsx:31-66`, `BackupsView.tsx:86-121`, `AuditView.tsx:30-65`  
~35 lines copy-pasted identically. Bug fix must be applied 4 times.  
**Fix:** Extract to `components/Pagination.tsx`.

### CLIENT-7: Data table components not memoized
**File:** `src/client/data-tables.tsx:9,87,149,186,309`  
`PostgresTargetTable`, `BackupTable`, `AuditTable`, `ContainerGroups`, `DeploymentTable` — all plain functions without `React.memo`.

### CLIENT-8: SettingsView receives 26 props
**File:** `src/client/views/SettingsView.tsx:8-36`  
Extreme prop drilling. State for cleanup/ssh/r2/cf lives in App but only used in Settings.  
**Fix:** Move settings-local state into SettingsView. Use Context for cross-cutting concerns (toast, confirm, busy).

### CLIENT-9: Over-fetching on view switch
**File:** `src/client/App.tsx:286-348`  
`loadView("projects")` fetches settings and nodes not needed for projects list.  
**Fix:** Fetch only what the target view needs.

### CLIENT-10: `loadAll()` refetches 10 resources after every mutation
**File:** `src/client/App.tsx:235-259`  
After saving a project, all 10 API endpoints are hit. Only one entity changed.  
**Fix:** Targeted invalidation — re-fetch only the resource that changed.

### CLIENT-11: `EnvEditor` defined inside App.tsx
**File:** `src/client/App.tsx:1700-1781`  
81-line component defined inside App module. Not memoized, can't be lazy-loaded, identity changes on HMR.  
**Fix:** Extract to `components/EnvEditor.tsx`.

### CLIENT-12: Settings-local state lives in App
**File:** `src/client/App.tsx:190-198,212`  
`cleanupLogs`, `cleanupLogTitle`, `cleanupPreviewed`, `systemLogs`, `sshPrivateKey`, `r2Form`, `cfForm` — all only used in SettingsView but cause App re-renders.  
**Fix:** Move into SettingsView. Pass initial values via props only.

### CLIENT-13: Inline closures in JSX defeat memoization
**File:** `src/client/App.tsx:1085,1092,1100-1111,1124`  
`onClick={() => setView(id)}` etc. — new closure per render, prevents future memoization of sub-trees.

### CLIENT-14: `will-change` missing on animated elements
**File:** `src/client/styles.css:2027-2035`  
`.spin` animation without `will-change: transform` — browser may not promote to compositor layer.

---

## Code Quality

### QUAL-1: 19 empty/silent catch blocks
**Files:** `services/audit.ts:31`, `services/projects.ts:46`, `services/cloudflare.ts:57,76,426,464,486,535`, others  
Audit log insertion failure silently swallowed. Docker errors silently return 0.  
**Fix:** At minimum, log errors. For audit, consider failing loudly.

### QUAL-2: Masked env values can be saved back as real values
**File:** `src/server/services/project-env.ts:100-107`  
Secrets >16 chars get partially masked (`AB****YZ`) which doesn't match sentinel, so `masked: false`. Frontend can save masked placeholder as actual value, destroying the secret.  
**Fix:** Use a consistent mask length or flag all non-empty masked values.

### QUAL-3: `normalizeString` defined identically in 3 files
**Files:** `services/settings.ts:43`, `services/cloudflare.ts:29`  
**Fix:** Extract to shared utility.

### QUAL-4: `runLogged` and `runLoggedOutput` are near-identical
**File:** `src/server/services/deployment-runner.ts:20-51`  
Copy-paste duplication. Bug fix in one missed in other.  
**Fix:** Merge into single function with `returnOutput` parameter.

### QUAL-5: `pathExists` defined in 2 places
**Files:** `services/paths.ts:54-61`, `services/ssh.ts:7-13`  
**Fix:** Import from `paths.ts`.

### QUAL-6: `rollbackTargetForProject` dedup logic is fragile
**File:** `src/server/services/deployments.ts:199-214`  
If two deployments share same `commitSha`, `distinct[1]` may not be what user expects.

### QUAL-7: `githubPayloadFromRequestBody` unguarded JSON.parse
**File:** `src/server/services/github-webhooks.ts:49-54`  
Malformed JSON → generic 500 instead of meaningful 400.

### QUAL-8: `deployments/logs` column has no DB-level size limit
**File:** `src/server/db/schema.ts:58`  
Truncation is application-level only. Runaway deployment → unbounded column growth.

### QUAL-9: `listAuditLogs` has no upper bound enforcement
**File:** `src/server/services/audit.ts:36-42`  
Function accepts any `limit`. Future caller could dump entire table.

### QUAL-10: `cleanupDocker` throws on partial failure
**File:** `src/server/services/docker.ts:186-197`  
If `image prune` succeeds but `container prune` fails, partial cleanup not recorded.

### QUAL-11: `runCommand` always resolves, never rejects
**File:** `src/server/services/commands.ts:20-92`  
Callers must check `exitCode`. Some don't (e.g., `systemUsage` for `df` command).

### QUAL-12: `clientDir` fallback may serve from nonexistent directory
**File:** `src/server/index.ts:102-105`  
If no candidate contains `index.html`, fallback `../client` still resolves with no error until request fails.

### QUAL-13: Top-level startup error doesn't close pool
**File:** `src/server/index.ts:135-138`  
`process.exit(1)` without `pool.end()` leaves idle PostgreSQL connections hanging.

---

## Test Coverage

### TEST-1: Zero route handler tests
All 9 route files in `src/server/routes/*.ts` have no HTTP integration tests. Auth, CRUD, CSRF — all untested.

### TEST-2: Core services untested
`backups.ts`, `cloudflare.ts`, `crypto.ts`, `deployment-runner.ts`, `nodes.ts`, `r2.ts`, `settings.ts`, `ssh.ts`, `github-webhooks.ts` — no tests.

### TEST-3: Auth middleware untested
**File:** `src/server/auth.ts`  
JWT signing, cookie management, `requireAuth` — no tests.

### TEST-4: Database migration untested
**File:** `src/server/db/index.ts:12-194`  
200 lines of raw SQL migrations with no test coverage.

### TEST-5: Zero React component tests
All 8 view files, all UI components — no tests.

### TEST-6: No integration/E2E tests
No test boots the server, connects to a database, or exercises any API endpoint end-to-end.

### TEST-7: Thin existing tests
- `deployments.test.ts` — only tests `recoverInterruptedDeployments` (1 test)
- `paths.test.ts` — 3 assertions only
- `tokens.test.ts` — `hashToken` untested

---

## Developer Experience

### DX-1: ESLint disables `no-explicit-any` globally
**File:** `eslint.config.js:13`  
Allows `any` everywhere, undermining type safety.

### DX-2: Double `server/` in output path
**File:** `tsconfig.server.json:9-10`  
`outDir: "dist/server"` + `rootDir: "src"` → `dist/server/server/index.js`. Works but fragile.

### DX-3: No vitest coverage configuration
**File:** `vitest.config.ts`  
No way to measure or enforce coverage thresholds.

### DX-4: No `dev:full` script for concurrent client+server
Developers must run two terminals. No `concurrently` setup.

### DX-5: `test` runs `vitest run` (no watch mode)
Default should be watch mode for local dev. `vitest run` better for CI.

### DX-6: No `test:watch` or `test:coverage` scripts
Missing convenience scripts.

### DX-7: No `dev:debug` script
No `--inspect` flag for attaching debugger to server.

### DX-8: No `.env.example` validation
24 variables listed, no script validates they're set. Config silently falls back to insecure defaults.

### DX-9: `drizzle-kit generate/push` scripts likely unused
Actual migration system uses raw SQL, not Drizzle migrations. Scripts are dead code.

### DX-10: No pre-commit hooks
No `.husky/` or `lint-staged`. Lint/typecheck not enforced before commits.

---

## Priority Roadmap

### Phase 1 — Critical Security (do now)

| # | Item | Effort | Impact |
|---|------|--------|--------|
| SEC-1 | Block startup on default credentials in production | Low | Prevents trivial compromise |
| SEC-3 | Add rate limiting to `/deploy` and `/webhooks/github` | Low | Closes brute-force vector |
| SEC-2 | Fix `constantTimeEqual` length leak | Low | Closes timing side-channel |
| SEC-4 | Validate/sanitize container IDs | Low | Prevents CLI flag injection |
| SEC-7 | Encrypt R2/Cloudflare secrets at rest | Medium | Protects cloud credentials |
| QUAL-2 | Fix masked env value saving bug | Medium | Prevents secret destruction |

### Phase 2 — Performance Quick Wins (this sprint)

| # | Item | Effort | Impact |
|---|------|--------|--------|
| PERF-1 | Wire SSE to EventEmitter bus instead of polling | Medium | Eliminates DB query storm |
| PERF-3 | Exclude `logs` from deployment list queries | Low | 10MB → ~50KB per API call |
| PERF-4 | Use `RETURNING` clause instead of SELECT after UPDATE | Low | Halves DB round-trips during deploys |
| CLIENT-4 | Remove `backdrop-filter` from `.panel` | Trivial | Eliminates GPU overhead |
| CLIENT-2 | Wrap handlers in `useCallback` | Medium | Makes `memo()` effective |
| PERF-5 | Cache container list with short TTL | Low | Eliminates redundant docker spawns |

### Phase 3 — Architecture Improvements (next sprint)

| # | Item | Effort | Impact |
|---|------|--------|--------|
| CLIENT-1 | Extract views from App.tsx into separate components | High | Eliminates full-tree re-renders |
| CLIENT-3 | Add `React.lazy()` code splitting | Low | Faster initial load |
| CLIENT-6 | Extract Pagination to shared component | Low | Eliminates 4x duplication |
| CLIENT-8 | Move settings-local state into SettingsView | Medium | Reduces App re-renders |
| ARCH-3 | Adopt Drizzle Kit migrations properly | High | Version-tracked, rollback-safe |
| QUAL-1 | Add logging to silent catch blocks | Medium | Surfaces hidden failures |

### Phase 4 — Testing & Quality (ongoing)

| # | Item | Effort | Impact |
|---|------|--------|--------|
| TEST-1 | Add route handler integration tests | High | Catches HTTP-level bugs |
| TEST-5 | Add React component tests | High | Catches UI regressions |
| TEST-3 | Add auth middleware tests | Medium | Catches auth bypass bugs |
| DX-4 | Add `dev:full` concurrent script | Low | Better DX |
| DX-5 | Default test to watch mode | Trivial | Faster dev cycle |
| DX-10 | Add pre-commit hooks | Low | Catches issues before commit |

### Phase 5 — Long-term Hardening

| # | Item | Effort | Impact |
|---|------|--------|--------|
| ARCH-2 | Run deployments in child process | High | Isolates API from deployment crashes |
| CLIENT-5 | Add SWR/React Query for data fetching | High | Eliminates redundant requests |
| SEC-5 | Make SSH host key checking configurable | Medium | Mitigates MITM risk |
| ARCH-4 | Configure DB pool limits | Low | Prevents connection exhaustion |
| DX-1 | Re-enable `no-explicit-any` lint rule | Medium | Improves type safety |

---

## Quick Reference: Files to Tackle First

1. `src/server/config.ts` — default credentials
2. `src/server/services/tokens.ts` — timing leak
3. `src/server/routes/workers.ts` — rate limiting
4. `src/server/services/docker.ts` — container ID sanitization
5. `src/server/routes/deployments.ts` — SSE → EventEmitter
6. `src/server/services/deployments.ts` — RETURNING clause, logs exclusion
7. `src/client/App.tsx` — useCallback, state extraction, code splitting
8. `src/client/styles.css` — backdrop-filter removal
9. `src/client/views/` — shared Pagination, memo cleanup
