# Yanto Initial Loading Performance Plan

**Date:** 2026-05-30  
**Scope:** First authenticated dashboard load and repeat dashboard refreshes  
**Goal:** Make the initial UI usable faster by reducing blocking Docker work, duplicate API work, and eager client bundle code.

---

## Current Bottlenecks

### 1. Dashboard triggers duplicate Docker inventory work
**Files:** `src/client/App.tsx`, `src/server/services/projects.ts`, `src/server/services/docker.ts`

The dashboard initial load calls:

- `api.projects()`
- `api.containers()`
- `api.nodes()`
- `api.systemUsage()`
- `api.settings()`

`api.projects()` currently calls `listProjectsWithContainerCounts()`, which also calls `listContainers()`. At the same time, the dashboard calls `/api/containers` directly. On a cold cache this can trigger duplicate Docker work.

**Impact:** First load waits on `docker ps` and `docker stats --no-stream` more than necessary.

### 2. `/api/containers` always fetches live stats
**File:** `src/server/services/docker.ts`

`listContainers()` runs:

- `docker ps -a --format "{{json .}}"`
- `docker stats --no-stream --format "{{json .}}"`

`docker stats --no-stream` is useful, but it is slower and not always needed for the first dashboard paint.

**Impact:** Dashboard cannot show the container inventory until live CPU/RAM stats return.

### 3. Initial dashboard uses many parallel API requests
**File:** `src/client/App.tsx`

Parallel requests are fine, but several are for data that could be returned together or loaded after the shell is visible.

**Impact:** More request overhead and harder server-side coordination of shared data like containers.

### 4. Modal/editor code is in the main client chunk
**File:** `src/client/App.tsx`

Views are already lazy-loaded, but `ProjectModal`, `CreatedProjectSecretModal`, `RollbackModal`, and `SetupWizardModal` are imported eagerly.

**Impact:** The main bundle includes code that is not needed for the first dashboard render.

---

## Phase 1: Remove Duplicate Docker Work

### 1. Make project listing Docker-free by default
**Files:**

- `src/server/services/projects.ts`
- `src/server/routes/projects.ts`
- `src/shared/types.ts`
- `src/client/views/ProjectsView.tsx`
- `src/client/views/DashboardView.tsx`

**Change:**

Stop calling `listContainers()` inside `listProjectsWithContainerCounts()` for the default `/api/projects` response.

Options:

- Rename it to `listProjectsWithRoutes()`.
- Return `containerCount: 0` or omit `containerCount` from the API response.
- Compute container counts on the client from the already-fetched `/api/containers` response.

**Preferred approach:**

Keep the API lightweight and compute counts client-side:

```ts
const projectContainerCounts = new Map<string, number>();
for (const container of containers) {
  if (!container.composeProject) continue;
  projectContainerCounts.set(container.composeProject, (projectContainerCounts.get(container.composeProject) ?? 0) + 1);
}
```

**Expected result:** Initial dashboard only calls Docker once.

### 2. Guard against concurrent `listContainers()` cache misses
**File:** `src/server/services/docker.ts`

**Change:**

Add an in-flight promise so concurrent requests share one Docker inventory run:

```ts
let containerCachePromise: Promise<ContainerInfo[]> | null = null;
```

If cache is stale and a fetch is already running, return the existing promise.

**Expected result:** Even if two endpoints call `listContainers()` at once, Docker is hit once.

---

## Phase 2: Split Container Inventory From Stats

### 3. Add a fast container listing mode
**Files:**

- `src/server/services/docker.ts`
- `src/server/routes/containers.ts`
- `src/client/lib/api.ts`

**Change:**

Support a lightweight mode that only runs `docker ps`:

- `GET /api/containers?stats=false`
- or a new `GET /api/containers/summary`

Return container identity, state, image, ports, compose labels, and timestamps. Fill stats with placeholders:

- `cpuPercent: "0%"`
- `memoryUsage: "-"`
- `memoryPercent: "0%"`

**Expected result:** Dashboard can render container counts and states without waiting for `docker stats`.

### 4. Load live stats after first paint
**File:** `src/client/App.tsx`

**Change:**

For dashboard load:

1. Fetch projects, nodes, settings, usage, and container summary.
2. Render immediately.
3. Fetch full `/api/containers` in the background and replace stats.

**Expected result:** Dashboard appears quickly; CPU/RAM stats fill in shortly after.

---

## Phase 3: Add A Dashboard Aggregate Endpoint

### 5. Add `GET /api/dashboard`
**Files:**

- `src/server/routes/system.ts` or new `src/server/routes/dashboard.ts`
- `src/server/index.ts`
- `src/client/lib/api.ts`
- `src/client/App.tsx`

**Response:**

```ts
{
  projects: Project[];
  deployments: Deployment[];
  nodes: DeploymentNode[];
  settings: SettingsState;
  usage: SystemUsage | null;
  containers: ContainerInfo[];
}
```

Use the fast container summary first. Full container stats can remain a background request.

**Expected result:** First dashboard load becomes one main API request plus one optional background stats request.

---

## Phase 4: Reduce Initial Client JS

### 6. Lazy-load modal components
**File:** `src/client/App.tsx`

**Change:**

Move these to `lazy()` imports:

- `ProjectModal`
- `CreatedProjectSecretModal`
- `RollbackModal`
- `SetupWizardModal`

Only load them when the corresponding modal state is open.

**Expected result:** Smaller main chunk and less parse/evaluation work before the dashboard appears.

### 7. Verify asset cache headers
**File:** `src/server/index.ts`

**Change:**

Serve hashed Vite assets with long cache headers:

```ts
app.use(
  "/assets",
  express.static(path.join(clientDir, "assets"), {
    immutable: true,
    maxAge: "1y"
  })
);
```

Serve `index.html` with `no-cache` so deployments still pick up new builds.

**Expected result:** Repeat visits load faster.

---

## Verification

### Measure API timing

Use curl against a production container:

```bash
time curl -sS http://localhost:8080/api/projects -H "Cookie: ..."
time curl -sS http://localhost:8080/api/containers -H "Cookie: ..."
time curl -sS "http://localhost:8080/api/containers?stats=false" -H "Cookie: ..."
```

### Add temporary server timing logs

Add short-lived timing around:

- `listProjectsWithContainerCounts()`
- `listContainers()`
- `docker ps`
- `docker stats`

Remove the logs after confirming the improvement.

### Browser checks

Run a production build and compare:

- Time until login/dashboard shell is visible
- Time until dashboard stat tiles render
- Time until container CPU/RAM stats fill in
- Network request count on first authenticated dashboard load
- Main JS chunk size after modal lazy-loading

---

## Suggested Implementation Order

1. Remove Docker dependency from `/api/projects`.
2. Add in-flight promise dedupe to `listContainers()`.
3. Add fast container summary mode.
4. Change dashboard to render from summary and refresh full stats in background.
5. Add `/api/dashboard` aggregate endpoint if request overhead is still visible.
6. Lazy-load modal components.
7. Add static asset cache headers.

---

## Success Criteria

- First dashboard render does not wait on duplicate Docker stats calls.
- Cold dashboard load uses at most one initial Docker inventory call.
- Container CPU/RAM stats can arrive after the dashboard is visible.
- Repeat visits reuse cached Vite assets.
- Main client chunk is smaller than the current `278 KB` build output.
