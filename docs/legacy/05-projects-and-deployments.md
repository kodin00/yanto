# Projects & Deployments

## Projects

The project is the central entity. It represents one deployable unit.

### Project fields

- `name`; `folderName` (filesystem-safe slug, unique across projects, used as the Compose project name and the on-disk directory name); `localPath` (absolute, derived from folderName under the projects root).
- **Source:** exactly one of `gitUrl` (a Git remote) or `dockerImage` (a Docker image reference). A free-form `source` string is normalized: if it parses as a Docker image reference it becomes a docker-image project; if it starts with `docker` it's rejected with a hint to use `docker pull ...`; otherwise it's treated as a Git URL.
- `branch` (default `master`); `composeFile` (default `docker-compose.yml`, validated relative `.yml/.yaml`); `composeContent` (optional pasted compose YAML, written to disk at deploy time); `envFile` (default `.env`).
- `autoStart`, `manualDeployEnabled` flags; `agentImage` (image for AI agent runs).
- `targetNodeId` (which deployment node runs this project; defaults to the local master node id).
- `deployToken` (random bearer token for webhook/manual deploy auth); `sshPrivateKeyPath`/`sshPublicKey` (managed SSH keys for Git access).

### Project → container mapping

A project maps to containers via the `folderName` ↔ `com.docker.compose.project` label match (Compose sets that label to the project's working-directory basename, which equals folderName). A project maps to deployments one-to-many via `deployments.projectId`.

### Lifecycle

- **Create:** ensures projects root exists; asserts the target node is deployable; slugifies/validates folderName; rejects duplicate folder names; checks compose port availability (against pasted content or on-disk file, ignoring the new project's own compose project name); inserts the row; generates a deploy token.
- **Update:** patches provided fields; re-resolves source if source/gitUrl/dockerImage changed; re-checks folder uniqueness on rename (and recomputes `localPath`); re-asserts deployable node on target change.
- **Delete:** takes an agent-lifecycle lock; refuses if any agent task is running; cleans up each task's worktree, prunes worktrees, deletes the DB row, then removes the project directory and the project's worktree directory from disk.
- **Read:** `listProjects`, `listProjectsWithContainerCounts` (joins container count by matching `container.composeProject === project.folderName`, plus Cloudflare routes), `getProject`, `getProjectDeployToken`.
- `publicProject` strips `deployToken` and `sshPrivateKeyPath` before returning to API consumers.

## Deployments

### Deployment entity

A deployment row: `id` (prefixed `dep`), `projectId`, `nodeId`, `status` (`running` | `success` | `failed`), `trigger` (`manual` | `webhook` | `github` | `rollback`), `targetRef` (optional git ref to deploy), `rollbackFromDeploymentId`, `commitSha`/`commitMessage` (filled in during run), `exitCode`, `logs` (append-only text, capped by `deploymentLogMaxChars` with a truncation notice), `startedAt`, `finishedAt`.

### Concurrency & state machine

- **One deployment per project may be `running` at a time.** Enforced via a Postgres advisory lock keyed on `deployment:<projectId>` inside a transaction that checks for an existing running deployment; if found, that existing deployment is returned (`reused:true`) and no new one starts.
- In-process, `activeDeployments` maps projectId → {deployment, AbortController, promise} for the local master only. A global `acceptingDeployments` flag gates new starts (set false on shutdown).
- Status transitions: `running` → `success` (exitCode 0) or `failed` (exitCode 1). On server startup, `recoverInterruptedDeployments` marks any still-`running` rows as `failed` with a notice that Yanto restarted.

### Trigger → completion flow (master node)

`startDeployment(projectId, trigger, options)`:
1. Reject if shutting down.
2. Inside a tx with advisory lock: if a running deployment exists for the project, return it reused. Otherwise load project + its target node. Refuse `pendingEnv` unless the node is the local master (env-write during deploy is master-only). Insert a new `running` deployment row.
3. If the target node is the local master: create an AbortController, store it in `activeDeployments`, and start `runLocalDeployment` (fire-and-forget). Otherwise: append a "queued for node X" log line (the worker will pick it up).
4. `runLocalDeployment` calls `runProjectDeployment`, then `finishDeployment(success,0)`, then reconciles Cloudflare tunnel assignments. On error: append a failure log line, `finishDeployment(failed,1)`. Always remove from `activeDeployments`.

### The deployment runner (`runProjectDeployment`) — step by step

Receives the project, the deployment row, callbacks (`appendLog`, `updateMetadata`, `signal`), and optional `pendingEnv`:

1. Resolve the Git SSH private key and build an SSH env (for git over SSH).
2. **Source checkout:** if `localPath` doesn't exist and there's a `gitUrl`, `git clone --branch <branch> <gitUrl> <localPath>`. If no gitUrl, just `mkdir -p` the folder. If the folder exists, leave it in place.
3. If the folder isn't a git repo but has a gitUrl, `git init`, `git remote add origin <gitUrl>`.
4. **Update ref:** if a `targetRef` was given (rollback or explicit ref), `git fetch --all --tags` then `git checkout <targetRef>`. Otherwise fetch the branch, checkout it (or create `-B` from `origin/<branch>` if it doesn't exist locally), then `git pull --ff-only`.
5. **Record metadata:** `git rev-parse HEAD` and `git log -1 --pretty=%s` → call `updateMetadata({commitSha, commitMessage, targetRef})`.
6. **Pending env:** if `pendingEnv` was supplied (master-only), write it to the env file (either raw text or key/value list) after checkout. This lets a deploy carry new env vars atomically.
7. **Resolve compose file:** use configured `composeFile` if it exists on disk; else if there's saved `composeContent` or a non-default compose file, keep the configured name; otherwise probe `compose.yml`, `compose.yaml`, `docker-compose.yaml`.
8. **Docker-image projects:** `docker pull <dockerImage>`.
9. **Compose content:** if `composeContent` is saved on the project, write it to `<localPath>/<composeFile>`. Else if it's a docker-image project, write the generated image compose. Then assert the compose file now exists (else a helpful error).
10. **Port check:** read the compose file content and assert ports available (ignoring the project's own compose project name).
11. **Auto-start override:** build compose args `["compose","-f",<composeFile>]`. If `autoStart`, write the restart-override file and append `-f <override>`; else delete the override file.
12. **Bring up the stack:** first `docker compose ... ps -q --status running` to count running containers. If any are already running, do `build` then `up -d --remove-orphans` (build-before-replace to minimize downtime). If none running, do `up -d --build --remove-orphans` (fresh build).

All commands are run through a logged runner that echoes `$ <cmd> <args>` to the log, streams stdout/stderr via a buffered log writer (with a 256KB pending cap and a "log skipped" notice if exceeded), checks the abort signal between steps, and on non-zero exit throws with the last 12 lines of output.

### Logs & streaming (SSE)

- `appendDeploymentLog(deploymentId, chunk)` appends to the `logs` column with a SQL-side truncation to `deploymentLogMaxChars` (keeping the tail + a truncation notice). After updating, it emits a `DeploymentLogEvent` on the in-process event bus.
- The event bus is keyed per deployment id. Listeners subscribe via `onLogUpdate` (returns an unsubscribe fn). `finishDeployment` emits a final event with `done:true`.
- SSE consumers subscribe to this bus and stream log+status updates to the browser; `done:true` closes the stream.

### Rollback

- `rollbackTargetForProject(projectId, deploymentId?, targetRef?)`: resolves the rollback target. If `targetRef` given, validate it (must be a safe git ref). Else if a `deploymentId` given, use that deployment's recorded `commitSha` (or `targetRef`). Else pick the **second distinct** previous successful deployment's ref (i.e. roll back one successful deploy, skipping the most recent). Throws if none available.
- `previewRollbackForProject(projectId, targetRef)`: requires an existing git checkout. Fetches all refs/tags, computes current HEAD summary and target summary, then `rev-list --count` in both directions (commits to apply / to leave behind) and `diff --numstat` for a per-file additions/deletions summary (capped at 40 files in the preview). Returns a `RollbackPreview`.

### Worker-side deployment flow

When the target node is a worker, the master only inserts the `running` row and logs "queued". The worker polls `/api/workers/jobs/next`, receives `{deployment, project}`, and runs the **same** `runProjectDeployment` locally on the worker, streaming logs/metadata/finish back to the master via authenticated HTTP PATCH/POST endpoints.

### Drain on shutdown

`drainActiveDeployments(timeoutMs)`: sets `acceptingDeployments=false`, aborts all active local controllers, and races `Promise.allSettled` of pending+active operations against a deadline, returning whether everything drained in time.

## Docker Management

Yanto drives Docker **entirely by spawning the `docker` CLI** (not the Docker socket HTTP API). All access is local to wherever the process runs (master node, or a worker node that has its own Docker daemon).

### Container listing (`listContainers`)

- Runs `docker ps -a --format {{json .}}`, then `docker stats --no-stream --format {{json .}}` and joins stats by container ID.
- Parses labels to extract `com.docker.compose.project` and `com.docker.compose.service`.
- Returns a normalized row per container: id, name, image, status, state, ports, createdAt (ISO-normalized), cpuPercent, memoryUsage, memoryPercent, composeProject, composeService, and a derived `isPostgresCandidate` flag (regex match on name/image/service for postgres/postgresql/postgis/timescale/pgvector/bitnami-postgresql).
- Sorted newest-first by createdAt.
- **Cached in-process with a 5s TTL** and a revision counter. Mutating operations call `invalidateContainerCache()` (bumps revision). Concurrent reads within the same revision share a single in-flight promise.

### Container control operations

`stopContainer`, `startContainer`, `restartContainer`, `execInContainer` — all first call `assertContainerCanBeControlled`, which inspects the container name and **refuses to act on Yanto's own protected containers** matching `^yanto-(app|postgres)-\d+$` (the platform's own app/db containers). Container IDs are validated against `^[a-zA-Z0-9][a-zA-Z0-9_.-]*$` before use. `execInContainer` runs `docker exec <id> sh -lc <command>`.

### Logs

`containerLogs` returns the last 500 lines via `docker logs --tail 500`.

### Cleanup

- `previewDockerCleanup` — read-only: `docker system df`, `docker builder du`, filtered `container/image/network ls` listings to show what would be reclaimed.
- `cleanupDocker` — a fixed conservative sequence: `docker builder prune -f`, `docker image prune -f`, `docker container prune -f`, `docker network prune -f`, plus `apt-get clean` if apt exists. It explicitly does **not** remove running containers or named volumes. Both invalidate the container cache on completion.

## Docker Compose Handling

Compose is treated as YAML text the user can either store in the repo or paste into the project editor.

- `buildDockerImageCompose(image)`: generates a minimal compose doc `{ services: { app: { image } } }` for Docker-image projects.
- `buildAutoStartOverride(composeContent)`: parses the compose YAML, lists every service name, and emits an override document that sets `restart: unless-stopped` on each service. This is written to a fixed file `.yanto.restart.override.yml` next to the project and passed as a second `-f` file to compose when auto-start is on; it is deleted when auto-start is off.
- `readProjectCompose(project)`: resolves the configured compose file under `project.localPath`; if the file is missing but the project is a Docker-image project, it synthesizes the image compose content; otherwise returns empty content with `exists:false`.

### Port-conflict checking (critical behavior)

- `extractPublishedComposePorts(composeContent)` parses each service's `ports` list. It handles both short string syntax (`"8080:80"`, `"3000-3002:3000-3002/udp"`, with bracket-aware splitting on `:`) and long object syntax (`{published: ..., target: ..., protocol: ...}`). It expands port ranges (capped at 512 ports, validated 1..65535). Ports containing `$` (env-interpolated) are skipped. Returns `{service, port, protocol, source}` per published host port.
- `findComposePortConflicts(content, {ignoreComposeProject})` detects:
  1. Intra-compose duplicates (same protocol:port used by two services).
  2. Host-port collisions with already-running containers (via `listContainers`, parsing each container's published ports from its `Ports` string) and with host processes listening on TCP (via `lsof -nP -iTCP -sTCP:LISTEN`, mapping port→process name). The `ignoreComposeProject` option excludes ports owned by the project's own already-running stack so a redeploy of the same project doesn't conflict with itself.
- `assertComposePortsAvailable` throws with a formatted human-readable message listing up to 8 conflicts if any are found. Called both at project create/update time and during deployment.

## Project Runtime & Environment

`project-runtime` provides compose-level **stop** and **restart** for an already-deployed project (no full redeploy):
- Resolves the compose file (configured, or `compose.yml` if present, else `docker-compose.yml`), asserts it exists, and appends the restart-override file as a second `-f` if it exists on disk.
- `stopProjectCompose` / `restartProjectCompose` run `docker compose ... stop|restart` in the project dir and invalidate the container cache.

`project-env` manages the project's `.env` file (or a configured alternative env file):
- Paths normalized via `normalizeEnvFile` (must be a relative path whose basename starts with `.env`).
- `readProjectEnv` / `readProjectEnvVariables` read and parse `KEY=VALUE` lines (skipping blanks and `#` comments; values are everything after the first `=`).
- `previewEnvContent` produces a line-by-line `EnvPreview` with line numbers, keys, `hasValue`, masked value, and comment text.
- `writeProjectEnv` writes raw text to the env file with mode 0600 (mkdir parents). `writeProjectEnvVariables` serializes a `{key, value, masked?}` list to `KEY=VALUE` lines and writes them.

## Nodes

A node is a deployment target. Roles: `master` (exactly one, the local node) or `worker`. Status: `online`/`offline`.

- `ensureLocalMasterNode`: idempotently upserts the local master node (`config.localNodeId`, default `node_master_local`) as role=master, status=online, refreshing `lastSeenAt`.
- `listNodes`: returns all nodes with computed status (master is always `online`; a worker is `offline` if `lastSeenAt` is older than 60s, else its stored status), plus aggregates `projectCount` and `runningDeploymentCount` per node. Ordered by role then name.
- `assertDeployableNode(id)`: loads a node and throws if missing (used at project create/update to validate `targetNodeId`).
- **Worker registration:** `registerWorker(input)` creates a worker node with a fresh id, a generated worker token, stores a hash of the token (HMAC verifier; the token itself is returned once to the caller and never stored). `nodeForWorkerToken(token)` recomputes the hash and looks up the worker node — the auth mechanism for worker API calls. `publicWorkerNode` strips `tokenHash` from any node before returning to clients.
- `markNodeSeen(node, input)`: updates `lastSeenAt=now`, status=online, and optionally name/dockerVersion/labels from the heartbeat payload.
- `nextWorkerDeployment(nodeId)`: oldest `running` deployment for that node joined with its project (the job served to a worker).
- Node labels are normalized to a flat `string→string` map. There is no node-level scheduling beyond FIFO by `startedAt` and the one-running-deployment-per-project constraint.

## Paths

Centralized, validated filesystem path computation. All folder/file name inputs are sanitized to prevent path traversal.

- `slugifyFolderName(input)`: lowercases, replaces non-`[a-z0-9._-]` with `-`, trims leading/trailing `-`, caps at 64 chars, defaults to `"project"` if empty.
- `normalizeFolderName(input)`: rejects empty, `..`, absolute paths, or anything outside `^[a-zA-Z0-9._-]+$`.
- `projectPath(folderName)`: `path.resolve(projectsRoot, safeName)` and asserts the result is inside `projectsRoot` (else throws).
- `normalizeComposeFile(input)`: defaults to `docker-compose.yml`; rejects absolute paths, `..`, or anything not matching `^[a-zA-Z0-9._/-]+\.ya?ml$`.
- `normalizeEnvFile(input)`: defaults to `.env`; rejects absolute/`..`/unsafe chars; requires the basename to start with `.env`.
- `ensureProjectsRoot`: `mkdir -p` the projects root.
- `removeProjectDirectory(folderName)`: `rm -rf` the resolved project path.
- `removeProjectWorktreeDirectory(folderName)`: resolves `<agentWorktreesRoot>/<folderName>`, asserts containment, and `rmdir`s it (only removes if empty; ignores ENOENT).

Roots come from config: `projectsRoot` (env `PROJECTS_ROOT`, default `/projects`), `agentWorktreesRoot` (env `AGENT_WORKTREES_ROOT`, default `<projectsRoot>/.yanto-worktrees`).

## Commands (subprocess abstraction)

The single subprocess abstraction everything else uses. `runCommand(command, args, options)` spawns a child process (no shell) and returns `{exitCode, output, timedOut?, truncated?}`.

- **Env merging:** by default merges `process.env` with the provided `env` (so spawned processes inherit the server env plus overrides). `inheritEnv:false` uses only the provided `env`.
- **Working directory** via `cwd`.
- **Output capture:** stdout and stderr are merged into one `output` string, byte-counted against `maxOutputBytes` (default 2MB). Once the cap is reached, further bytes are dropped and `truncated:true` is set, but `onData` callbacks still receive every chunk (so live log streaming is uncapped while stored output is capped).
- **Streaming:** `onData(chunk)` is invoked for every stdout/stderr chunk as text — how the deployment runner streams logs in real time.
- **Timeout:** `timeoutMs` (default 1h). On timeout: SIGTERM, then SIGKILL after 5s; report exitCode 124 with a "timed out" notice.
- **Abort:** if an `AbortSignal` is provided and aborts, SIGTERM, then SIGKILL after 5s. Honors pre-existing abort.
- **Process groups:** when `killProcessGroup:true`, spawns detached and kills the whole process group (negative pid) on timeout/abort — important so `docker compose build` and child build steps die together.
- `runCommandChecked`: convenience wrapper that throws on non-zero exit and returns the output.

## Worker Runtime (HTTP client helper)

- `requestWorkerJson(url, options, timeoutMs)`: fetch with a combined abort signal (caller signal + timeout). Parses JSON error bodies (`{message}`) on non-OK responses; returns `undefined` for 204; distinguishes a timeout abort (from the internal timer) from a caller abort.
- `waitForWorkerPoll(delayMs, signal)`: an abortable sleep used to pace the worker loops.

## Cross-cutting data flow summary

- **Persistent state:** PostgreSQL. All concurrency control for deployments is Postgres advisory locks + the one-running-per-project rule.
- **Project → containers:** by `folderName` ↔ compose `com.docker.compose.project` label. Container listing is cached 5s and invalidated on any mutation.
- **Deploy → compose:** a deploy clones/updates the repo at `localPath`, optionally writes env and compose content, checks host-port conflicts, applies a restart-override file when auto-start is on, then `docker compose up -d --build --remove-orphans` (or build-then-up if containers already running).
- **Master vs worker:** identical `runProjectDeployment` logic runs in both; the difference is where it executes and how logs/metadata/finish are reported (direct DB writes on master, authenticated HTTP back to master on worker). The master owns the queue; workers pull FIFO.
- **Events:** an in-process EventEmitter fans deployment log/status updates to SSE clients; `done:true` marks completion.
- **Subprocess discipline:** one `runCommand` abstraction with output capping, streaming, timeouts, abort, and process-group kills — used by docker, git, and the deployment runner uniformly.
