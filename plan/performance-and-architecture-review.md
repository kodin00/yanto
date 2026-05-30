# Yanto: Performance & Architecture Review

**Date:** 2026-05-30
**Scope:** Full project audit covering recent changes (setup wizard, boot loader), architecture, performance, and testability.

---

## Critical Bugs (fix now)

### 1. Race condition: "Finish" and "Skip" buttons are independently disabled, both can fire concurrently

**File:** `src/client/App.tsx:1813-1828`

The Skip button is disabled only when `busy === "setup-dismissed"` and Finish only when `busy === "setup-completed"`. Clicking Finish sets `busy` to `"setup-completed"`, which disables Finish but leaves Skip fully clickable. A second click on Skip fires `saveSetupWizard("dismissed")`, overwriting `busy` and racing the in-flight "completed" request.

Additionally, `closeSetupWizard` (line 976) is wired to the Modal's `onClose` and does not check `busy` at all. Pressing Escape after clicking Finish fires a second "dismissed" request that races the "completed" one.

**Fix:** Disable all wizard action buttons when any save is in flight. Use a single boolean `busy` check (e.g., `busy?.startsWith("setup")`) rather than exact string matching.

---

### 2. Server-side `saveSetupWizardSettings` uses non-atomic read-then-write

**File:** `src/server/services/settings.ts:129-147`

The function reads current state, merges locally, then upserts. No transaction or `SELECT ... FOR UPDATE` wraps the read+write. Two concurrent requests (triggered by the client race above, or rapid double-clicks) both read the same stale snapshot and the last writer wins -- silently losing one action.

**Fix:** Use a single SQL upsert that sets fields conditionally:

```sql
INSERT INTO app_settings (key, value, updated_at)
VALUES ('setup.wizard', $json, now())
ON CONFLICT (key) DO UPDATE SET
  value = jsonb_build_object(
    'completedAt', COALESCE(EXCLUDED.value->>'completedAt', app_settings.value->>'completedAt'),
    'dismissedAt', COALESCE(EXCLUDED.value->>'dismissedAt', app_settings.value->>'dismissedAt'),
    'updatedAt', now()
  )::text,
  updated_at = now()
```

Or wrap the read+write in a database transaction.

---

### 3. No escape hatch when setup wizard API call fails

**File:** `src/client/App.tsx:976-982`

When the user clicks X and neither `completedAt` nor `dismissedAt` is set, `closeSetupWizard` calls `saveSetupWizard("dismissed")`. If the API fails (network error, 500), the catch block shows a toast but the modal stays open. Every subsequent X or "Skip" click retries the same failing call. The user must reload the page to escape.

**Fix:** Add a "Close anyway" path that sets `setSetupModalOpen(false)` even if the API call fails, or allow closing the modal freely and only persist the status on success.

---

## Performance Issues (fix soon)

### 4. `loadAll` fetches every resource after every mutation

**File:** `src/client/App.tsx:235-259`

After saving a project, deploying, creating a backup, saving SSH keys, or running cleanup, the code calls `loadAll()` which fires 10 parallel API requests (projects, deployments, containers, nodes, backups, postgresTargets, auditLog, systemUsage, settings, systemLogs). Only one entity actually changed.

A 30-second `setInterval` (line 338) also calls `loadView(view)` which fetches a large overlapping subset.

**Fix:** Replace `loadAll()` calls with targeted invalidation -- re-fetch only the resource that changed. For example, after saving a project, call `api.projects()` only. Keep the 30s poll for the current view but make it fetch the minimal set.

---

### 5. `listProjectsWithContainerCounts` spawns a Docker IPC call on every invocation

**File:** `src/server/services/projects.ts:34-55`

This function calls `listContainers()` (Docker CLI spawn) every time. It's hit by `/api/projects` on every dashboard load and 30-second poll. The same `listContainers()` is also called separately via `/api/containers` on the same poll cycle -- Docker is queried twice per poll.

**Fix:** Cache the Docker container list with a short TTL (e.g., 5-10 seconds) on the server. Or compute container counts client-side from the already-fetched container list.

---

### 6. SSE log stream uses database polling at 700ms intervals

**File:** `src/server/index.ts:805-863`

The `/api/deployments/:id/logs/stream` SSE endpoint polls the database every 700ms with `setInterval`. Each client viewing deployment logs creates a polling loop. With 5 concurrent users, that's ~7 database queries/second just for log streaming.

**Fix:** Use PostgreSQL `LISTEN/NOTIFY` or an in-memory pub/sub to push changes only when new log content arrives.

---

### 7. Boot loader animation allocates two 1776-element arrays per frame at ~30fps

**File:** `src/client/components/YantoBootLoader.tsx:9-11`

Every ~33ms, `renderFrame` allocates fresh `chars` and `zBuffer` arrays (74x24 = 1776 elements each), uses them once, and discards them. On low-end devices, this GC pressure during the initial load screen degrades perceived performance.

**Fix:** Hoist the arrays outside `renderFrame` and reset them in-place each frame, or use a `useRef` to persist them across renders.

---

## Architecture Improvements (plan for next sprint)

### 8. Monolithic `App.tsx` -- 2,330 lines, 40+ useState hooks, 30+ handlers

**File:** `src/client/App.tsx`

The entire client app lives in one component. Every state change (toast, busy, form field) re-renders the entire tree -- no `React.memo` boundaries exist. Adding a feature means modifying this 2,300-line file.

**Plan:** Extract view-level components:
- `DashboardView` (stats, warnings, setup banner)
- `ProjectsView` (project list, create/edit modals)
- `SettingsView` (SSH, CF, R2 forms, setup wizard trigger)
- `DeploymentsView`, `ContainersView`, `BackupsView`, `AuditView`

Each gets its own state and data-fetching. Use `React.memo` on table rows and list items. Keep the top-level `App` as a router/shell only.

---

### 9. Server entry point is a 1,226-line route file with all handlers inline

**File:** `src/server/index.ts`

~50 route handlers registered on a single `app` object with no router separation. Business logic is properly in services, but the HTTP layer is monolithic.

**Plan:** Split into Express Router modules by domain:
- `routes/projects.ts`
- `routes/deployments.ts`
- `routes/settings.ts`
- `routes/containers.ts`
- `routes/backups.ts`
- `routes/workers.ts`
- `routes/cloudflare.ts`
- `routes/auth.ts`

Each router owns its own middleware (auth checks, validation) and delegates to services.

---

### 10. Zero client-side test coverage

**Files:** `tests/` (10 files, all server-side)

No tests exist for React components, the API client, utility functions, or the client-server contract. The 40+ state variables with cross-dependencies in `App.tsx` have no safety net.

**Plan:**
- Add Vitest + React Testing Library
- Start with utility tests (`app-utils.ts` -- pure functions, easy wins)
- Test the API client layer (mock fetch)
- Integration tests for key flows: login, create project, deploy, setup wizard

---

## Minor / Cosmetic

### 11. `backdrop-filter: blur(14px)` on opaque background does nothing but forces GPU compositing

**File:** `src/client/styles.css:161`

`.boot-loader__ascii` has `background: var(--panel)` (opaque) and `backdrop-filter: blur(14px)`. The blur has nothing to blur but forces a compositing layer. Remove the `backdrop-filter` line.

### 12. `settingsLoaded` never set if `api.settings()` throws

**File:** `src/client/App.tsx:245-257`

If `api.settings()` rejects, `Promise.all` in `loadAll` rejects and `setSettingsLoaded(true)` never runs. The setup wizard auto-prompt and reopen banner silently disable for the session with no feedback. Add `.catch()` to `api.settings()` or set `settingsLoaded` in a `finally` block.

---

## Priority Order

| # | Severity | Effort | Item |
|---|----------|--------|------|
| 1 | Critical | Low | Fix button/race in setup wizard client |
| 2 | Critical | Medium | Atomic write in `saveSetupWizardSettings` |
| 3 | Critical | Low | Escape hatch for failed wizard API call |
| 12 | High | Low | `settingsLoaded` in finally block |
| 4 | High | Medium | Targeted API invalidation instead of `loadAll` |
| 7 | Low | Low | Hoist boot loader arrays |
| 11 | Low | Trivial | Remove useless `backdrop-filter` |
| 5 | Medium | Medium | Cache Docker container list |
| 6 | Medium | High | LISTEN/NOTIFY for SSE log stream |
| 8 | High | High | Split App.tsx into view components |
| 9 | Medium | Medium | Split server routes into routers |
| 10 | High | High | Add client-side test coverage |
