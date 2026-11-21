# Frontend UI Surface

The goal of this doc is to enumerate the user-facing features/screens so a reimplementation knows what UI to build. The client is a React 19 SPA served as static assets by the server.

---

## API Client (`src/client/lib/api.ts`)

A single `api` object wrapping `fetch` with:
- JSON content type default.
- **CSRF token** from `yanto_csrf` cookie sent as `X-CSRF-Token`.
- Credentials included.
- 401 → unauthorized handler.
- GET requests are **deduplicated per generation** (in-flight GETs with the same URL share a promise); any non-GET mutation **invalidates** cached GETs so a follow-up refresh re-fetches.
- Errors parsed from JSON `{ message }` or text/plain (truncated to 500 chars).

### Endpoint groups the frontend calls

- **Setup/auth:** setup status, turnstile, setup owner, login, logout, me, account setup preview/setup.
- **Users (owner admin):** list, create member + setup URL, replace access, set status, reset link, delete.
- **Projects:** list, create (returns deploy token), update, delete, get deploy-token, deploy (optional env payload), stop, restart, rollback preview, rollback, env read/content/preview/write/patch, compose content, agent-branches, cf-routes.
- **Nodes (multi-node):** list, join-token.
- **AI / Codex:** providers, codex status, login start/cancel, logout, models refresh, provider CRUD, discover, model CRUD.
- **Agent tasks:** list (active/archived), worktrees, task CRUD, run, stop, git preview, commit, push, worktree cleanup, **SSE** events stream.
- **Deployments:** paginated list, logs (text), **SSE** logs stream.
- **Backups:** paginated list, postgres-targets, create, restore (raw octet-stream + X-Filename), delete, upload-r2, download.
- **Audit:** paginated list (filter by projectId).
- **Containers:** list, logs (text), **SSE** logs stream, stop/start/restart, exec.
- **System:** usage, logs (text), cleanup preview/run.
- **MCP tokens:** list, create, delete.
- **Settings:** aggregated read, runtime/r2/cloudflare/ssh-key/setup-wizard/multi-node save, cloudflare validate.
- **Cloudflare:** tunnels/clients/assignments/hostnames CRUD, per-node tunnel control, DNS records (global + per-client), routes diagnostics, project routes enable/disable/delete.
- **FRP:** overview, client-setup, settings, server start/stop/restart, tunnel CRUD.

---

## Views (`src/client/views/`)

The app has **12 top-level screens** (the `AppView` enum): `dashboard | projects | tasks | deployments | containers | nodes | backups | hostnames | frp | dns | audit | settings`.

### DashboardView.tsx
Main dashboard: project/node/container/running-deployment counts, system usage, setup-wizard reopen button, failing-projects / unhealthy-containers / warning-disks alerts, recent deployment table.

### ProjectsView.tsx
Projects list/grid with per-project deployment & container status badges, deploy/stop/restart/rollback actions, deploy-token copy, GitHub webhook endpoint copy, edit/create modal, pagination.

### TasksView.tsx
AI agent task board with 4 columns (Backlog/Running/Review/Done) plus tabs for Archive, Worktrees, and AI Providers; create/run/stop/commit/push/cleanup tasks; live event stream; OpenAI/Anthropic provider config.

### DeploymentsView.tsx
Paginated deployment history table with log viewing and retry.

### ContainersView.tsx
Docker container list grouped by project, with logs/terminal openers and start/stop/restart controls.

### NodesView.tsx
Multi-node table (name, role, status, Docker version, project count, running deployments, last seen).

### BackupsView.tsx
Backup list + Postgres target list; dump app DB or a container, restore a dump into a container (file upload), upload to R2, download, delete.

### HostnamesView.tsx
Cloudflare Tunnel management with tabs (hostnames/tunnels/clients); owner vs project-scoped variants; create clients/tunnels/assignments/hostnames, retry/delete.

### DnsView.tsx
Cloudflare DNS record manager (A/AAAA/CNAME/TXT/MX/NS editable; proxied for A/AAAA/CNAME) with TTL/priority/comment, route diagnostics, pagination.

### FrpView.tsx
FRP (fast reverse proxy) overview: server start/stop/restart, client setup info, tunnel CRUD (tcp/udp, local host/port, remote port, enabled toggle).

### AuditView.tsx
Paginated audit log table.

### SettingsView.tsx
Owner settings hub: runtime settings (appBaseUrl, cookie secure, Turnstile, command timeouts/log limits), R2 settings, Cloudflare settings (+ validate), SSH key save/generate, MCP token create/revoke, multi-node toggle, setup-wizard, system logs viewer, cleanup preview/run, user access panel.

---

## Components (`src/client/components/`)

### ui.tsx
Shared design-system primitives: Button (primary/secondary/danger/ghost, loading), IconButton, StatusBadge, TextField, TextAreaField, ToggleField, CustomSelect, Modal (with focus-trap/modal-stack), ConfirmDialog, LogViewer, LoadingInline.

### Pagination.tsx
Reusable prev/next paginator with "X-Y of N" label, hidden when items fit one page.

### EnvEditor.tsx
Project `.env` editor with two modes: key/value pairs (add/remove rows) and raw text; serializes/parses `KEY=value` lines, ignores comments/blanks.

### ContainerTerminal.tsx
Modal terminal that runs one-off `docker exec` commands per submission (no persistent shell state), with command history (up/down arrow recall) and exit/truncation/timeout notices.

### UserAccessPanel.tsx
Member project-access editor with presets (viewer=none, operator=deploy+runtime+backups, manager=all) and per-project custom permission toggles; create members, copy reset links, enable/disable, delete.

### YantoBootLoader.tsx
Animated ASCII boot/loading splash rendering a rotating "YANTO" core (74x24) frame-by-frame.

---

## Hooks

### useOperationalHistory.ts
Shared paginated-data hook for the three "history" views — **deployments**, **backups**, and **audit**. Holds rows + totals + current page for each, exposes `load*`, `refresh*`, and `select*Page` callbacks that coordinate loading/error state via injected setters and a `refresh` wrapper. Backups load also fetches Postgres targets.

---

## ASCII Art (`src/client/ascii/`)

- **types.ts** — `Point3D` and `PlotOptions` (width/height/chars/zBuffer/scaleX/scaleY) types for the ASCII renderer.
- **asciiRenderer.ts** — 3D point rotation (pitch/yaw/roll) + perspective projection plotter with a z-buffer (depth = `5 + z`, perspective `5/depth`), writing a single char into a char buffer + z-buffer.
- **drawYantoCore.ts** — Draws the animated "YANTO" boot animation: a rotating ring of 128 points, drifting sparks, and the 3D letterforms, all plotted through the renderer.

---

## View/permission model (for the UI)

Members get a base set of views (`dashboard`, `projects`, `deployments`, `containers`, `audit`) plus `backups`/`hostnames` if granted those project permissions. The owner sees all views. The UI should derive the visible navigation from the session user's `allowedViews` (returned by `/api/auth/me`).
