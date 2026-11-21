> Classification: INTERNAL

# 04 — Feature specifications

Terms follow [`CONTEXT.md`](../../CONTEXT.md); decisions D1–D21 in [`00-scope.md`](00-scope.md) and the ADRs are binding and not re-decided here. Legacy behavior: [`docs/legacy/`](../legacy/).

**Conventions**
- **Fact** = grounded in the legacy code/docs or in documentation fetched via Context7 on 2026-11-21. **ASSUMPTION** = proposal not grounded in a source. **unverified** = a claim not confirmed in a source available while writing; check before implementing.
- Host side effects list files, commands and API calls exactly. "Docker" means the Docker Engine API through `bollard` unless a CLI is named. `docker compose` is always the CLI (the Compose plugin has no Engine API).
- Paths use `DATA=/var/lib/yanto` (default data dir; layout canonical in 01 §8.2). `RUN=/run/yanto` is the systemd `RuntimeDirectory` (root-only tmpfs, 0700) for transient files.
- Every subprocess is spawned without a shell, in its own process group, with a timeout and a cancellation token (legacy `runCommand` discipline: SIGTERM, then SIGKILL after 5 s). Output is streamed to a log sink and capped when stored.
- Secrets (Deploy Tokens, Registry Credentials, Cloudflare API tokens, frps tokens, Deploy Key private keys) are stored encrypted at rest (AES-256-GCM envelope, key `/var/lib/yanto/keys/app.key`; design in 01 §8.3, columns in 02 §3), except frps tokens, which stay only in the operator's `frps.toml` and never written to logs, audit metadata, or process arguments.
- Every mutating operation writes an Audit log entry (§10). Failures surface in the htmx UI as an inline error banner with the step name and last 12 output lines, and in REST/MCP as the uniform error body of 03 §c with the same text.

---

## 1. Projects & Deployments

### 1.1 Behavior
A Project is Git-backed or image-backed and runs under Docker Compose with project name = `slug`. A Deployment brings it to a revision (Git ref, or image reference) and records status, logs, and the resolved revision.

**Source rules**
- Exactly one of `git_url` / `image_ref` (02 §4.2 `projects`). Free-form input is classified as in legacy: parses as an image reference → image Project; starts with `docker ` → rejected with hint; else Git URL.
- Git URL schemes allowed: `ssh://`, scp-style `user@host:path`, `https://` (public repos only; no credentials in URLs — ASSUMPTION, avoids token leakage into `.git/config`). Reject anything else (`file://`, `ext::`, leading `-`).
- `branch` default `master` (ASSUMPTION: also accept configured default `main`; no auto-detection). Refs must match `^[A-Za-z0-9._/@-]+$`, not start with `-`, not contain `..`.
- `compose_path` default `docker-compose.yml` (`^[A-Za-z0-9._/-]+\.ya?ml$`, relative, no `..`); `env_file` default `.env` (relative, basename starts with `.env`); `slug` per 02 (`[a-z0-9][a-z0-9_-]*`, ≤ 63, unique); `compose_content` optional pasted YAML (overrides repo file at deploy); `auto_start` (restart policy override); `manual_deploy_enabled`; `github_webhook_enabled`.
- Image Projects without `compose_content` get a generated compose `services: { app: { image: <ref> } }`.

**Declared ports (Change vs legacy)**
- A Project's published ports are *declared* state: `project_ports` (02 §4.2: `label`, `compose_service`, `container_port`, `host_port`, `bind_address`, `protocol`). They are derived on every save/deploy from `docker compose -p <slug> -f <file> config --format json` (ASSUMPTION: flag and JSON shape per the installed Compose v2; fall back to YAML parsing of `ports` short/long syntax with range expansion capped at 512 as in legacy; `${VAR}` unresolved ports are reported as warnings, not silently skipped).
- Only host IPs `127.0.0.1` / `::1` are accepted. A mapping with no host IP or a non-loopback host IP is rejected with "publish on 127.0.0.1 so an Ingress Provider carries the traffic". Rationale: public publish would bypass Hostname routing and expose the port directly (access control).
- Hostnames (§9) may target only declared ports.

**Collision check** (runs at create/update, pre-deploy, and when a Hostname targets a port). A declared `host_port/protocol` conflicts if it is: (a) declared twice inside the Project; (b) declared by another Project; (c) published by a running container not belonging to this Project (`list_containers` ports, ignoring containers whose `com.docker.compose.project` equals this Project's `slug`); (d) bound by any host listener (parse `/proc/net/tcp`, `/proc/net/tcp6` rows in state `0A`; map socket inode → PID → `comm` via `/proc/*/fd`, best effort; no `lsof` dependency in a static binary). Error lists up to 8 conflicts with owner (Project/container/process).

**Queue and concurrency**
- Statuses: `queued | running | success | failed | cancelled | interrupted`. At most one `running` Deployment per Project (enforced by a partial unique index `WHERE status='running'` plus an in-process per-Project mutex; SQLite replaces the legacy advisory lock).
- A trigger arriving while one is running enqueues one Deployment; a further trigger while one is queued returns the queued one (`reused: true`, same as legacy reuse semantics). Queue is FIFO across Projects with a global concurrency cap `deploy.max_concurrent` (`app_settings`, default 2, ASSUMPTION).
- Startup recovery: `running` rows become `interrupted` with notice "Yanto restarted during this Deployment"; `queued` rows resume.
- Shutdown: stop accepting, wait up to 30 s for running Deployments (legacy default), then cancel; the systemd unit sets `TimeoutStopSec=45` (05 §5).

**Cancellation**: `queued` → `cancelled` immediately. `running` → trip the cancellation token; current child group gets SIGTERM, SIGKILL after 5 s; status `cancelled`, exit code 130 (ASSUMPTION). No automatic cleanup: a cancelled `compose up` may leave a partial stack; the log says so and offers Rollback/Redeploy.

**Rollback**
- Git Project: target = explicit ref, else a chosen prior Deployment's `commit_sha`, else the second distinct successful commit (legacy rule). Preview (read-only, needs existing checkout): `git fetch --all --tags`, current vs target summary, `git rev-list --count` both directions, `git diff --numstat` capped at 40 files.
- Image Project (New): each successful Deployment records `image_digest` (`RepoDigests[0]` from `inspect_image`). Rollback deploys `repo@sha256:<digest>` via a generated override; preview does a manifest existence check (for Hosted Registry images the digest may have been garbage-collected, §4) and fails early if missing.
- Rollback creates a new Deployment with trigger `rollback` and `rollback_from_deployment_id` (02 §4.2). Per D25 every Deployment snapshots the compose file it ran (`compose_snapshot`) and the encrypted `.env` (`env_snapshot_enc`), and rollback restores that exact config together with the code/image. The preview shows a compose diff and an env key-level diff (values masked).

### 1.2 Inputs / state (SQLite)
- Tables (all defined in 02 §4): `projects` (incl. `deploy_token_hash`, `deploy_token_enc`, `deploy_key_id`, `registry_credential_id`), `deploy_keys` (shared rows, referenced by Projects), `deployments` (trigger `manual|api|deploy_token|github|rollback|cron`; `rollback_from_deployment_id`, `image_digest`, `compose_snapshot`, `env_snapshot_enc`), `deployment_logs(deployment_id, seq, ts, stream, data)` capped at `runtime.deployment_log_max_bytes` (default 500 000, legacy): oldest chunks dropped, a truncation notice kept at the head.
- Files on Host: `DATA/projects/<slug>/` (mode 0750), `<compose_path>`, `<env_file>` (0600), `.yanto.restart.override.yml`.

### 1.3 Host/Docker side effects (exact order of a Deployment)
1. Insert `deployments` row (`queued`), then `running` when dequeued. Emit SSE `status`.
2. **Pre-flight**: `docker compose version` (cached 60 s); ensure `DATA/projects` exists.
3. **Git source** (if `git_url`):
   - Env for every git command: `GIT_TERMINAL_PROMPT=0`, `GIT_ALLOW_PROTOCOL=ssh:https`, and, if the Project has a Deploy Key, `GIT_SSH_COMMAND="ssh -i DATA/ssh/<slug>/id_ed25519 -o IdentitiesOnly=yes -o BatchMode=yes -o StrictHostKeyChecking=accept-new -o UserKnownHostsFile=DATA/ssh/known_hosts"`. The private key is materialized to `DATA/ssh/<slug>/id_ed25519` (0600, parent 0700; 01 §8.2) only for the duration of the Deployment and removed after (decrypted from `deploy_keys.private_key_enc`), never placed in argv. `StrictHostKeyChecking` is configurable (`accept-new` default, `yes` for pre-seeded known_hosts).
   - Missing dir: `git clone --branch <branch> -- <git_url> <path>`. Dir exists but no `.git`: `git init` + `git remote add origin <git_url>`.
   - Explicit target ref: `git fetch --all --tags`, `git checkout <ref>`. Otherwise `git fetch origin <branch>`, `git checkout <branch>` (or `checkout -B <branch> origin/<branch>`), `git pull --ff-only`.
   - `git rev-parse HEAD`, `git log -1 --pretty=%s` → stored on the row.
4. **Config files**: write `compose_content` (if set) to `<path>/<compose_path>`; for image Projects without content write generated compose. Write pending env (from `deployments.pending_env_enc`, if the trigger carried one) to `<env_file>` mode 0600 via temp file + rename. Assert compose file exists (else error naming the probed alternatives `compose.yml`, `compose.yaml`, `docker-compose.yaml`). Store the compose content and the encrypted `.env` content on the Deployment row (D25).
5. **Registry auth** (if the Project has a Registry Credential or its image is on the Hosted Registry): create `RUN/docker-<deployment_id>/` (0700) containing `config.json` with `auths` entries only, symlink `cli-plugins` to the system/real plugin dir if present; export `DOCKER_CONFIG=<that dir>` for steps 6 and 8; remove the dir in a drop-guard (also swept on startup). Details and justification: §4.7.
6. **Image Projects**: `docker pull <image>` (or `docker compose pull` for Git Projects whose compose names private images).
7. **Ports**: derive declared ports, run the collision check, reconcile with `project_ports` (a mismatch with the declared set fails the Deployment).
8. **Compose**: args `compose -p <slug> -f <compose_path> [-f .yanto.restart.override.yml] [--env-file <env_file> if non-default]`. Auto-start on → write the override (`restart: unless-stopped` for every service, names from `compose config --services`), off → delete it. `... ps -q --status running`: if any running → `build` then `up -d --remove-orphans`; none → `up -d --build --remove-orphans`.
9. Record `image_digest` (image Projects), set `success`/`failed`, emit SSE `done`, write Audit entry, invalidate the container cache (§2), trigger Hostname reconcile if declared ports changed (§9).

Per-step timeouts (ASSUMPTION): git 10 min, pull 30 min, build/up 60 min. Each command line is echoed to the log as `$ <cmd> <args>` (env values never echoed).

**Project lifecycle side effects**
- Create: dir + files as above, generate Deploy Token (32 random bytes, base64url, prefix `ydp_`; stored as hash + encrypted copy, 02 §3) and optionally a Deploy Key (ed25519 generated in-process with the `ssh-key` crate, 01 §11; private half stored encrypted in `deploy_keys`).
- Stop/restart (no redeploy): `docker compose -p <slug> -f ... stop|restart`.
- Delete: refuse while `running`; `docker compose -p <slug> down --remove-orphans` is **not** automatic — UI offers "also stop containers" (default on) and "also delete volumes" (default off, typed-name confirmation); then delete Hostnames (§9), rows, and `rm -rf` the resolved project dir (path-containment asserted under `DATA/projects`).

### 1.4 Webhooks
- **Manual/CI**: `POST /deploy?id=<project_id>`, `Authorization: Bearer <deploy_token>`; body ignored (no ref/env via Deploy Token, 03 §a.6). Requires `manual_deploy_enabled`. Constant-time compare of SHA-256 against `deploy_token_hash`. Response `202 {deployment, reused}`. Not CSRF-protected (bearer-only; no cookie auth accepted on this route). Rate limits per 03 §a.4.
- **GitHub**: `POST /webhooks/github?id=<project_id>`. Verify `X-Hub-Signature-256: sha256=<hex>` = HMAC-SHA256(secret = the Project's Deploy Token, body = raw bytes, before any form decoding), constant-time compare; missing/invalid → 401. Then: `X-GitHub-Event: ping` → 200; not `push` → 202 ignored; `deleted: true` → 202 ignored; ref not `refs/heads/<project branch>` → 202 ignored; `github_webhook_enabled=false` → 202 ignored; else enqueue with trigger `github`; the Deployment uses the branch head, not the pushed `after` sha (legacy behavior). Form-encoded delivery (`payload=` field) is unwrapped after verification. Dedupe on `X-GitHub-Delivery` (last 100 ids in memory, 03 §a.6) → duplicate returns 200 `duplicate`. Body cap 1 MiB → 413.
- Image Projects: the same endpoints pull the current tag and redeploy (typical CI flow: push to Hosted Registry, then call `/deploy`).
- Deploy Token reveal requires `secrets`; rotate invalidates the old token and the GitHub secret simultaneously (UI says so).

### 1.5 Logs and SSE
- Writer batches log text every 250 ms or 64 KiB into `deployment_logs` and publishes on an in-process `tokio::broadcast` per Deployment.
- `GET /deployments/{id}/logs/stream` (`text/event-stream`, 03 §b): `event: log` (`id: <seq>`), `event: status`, `event: done`. `Last-Event-ID` replays from the DB; a lagged receiver resyncs from the DB. Keep-alive comment every 15 s. Stream closes after `done`.
- Log text is HTML-escaped on render (logs are untrusted: build output, git messages); ANSI colors are converted to a fixed class whitelist.
- Redaction: known secret values (Deploy Token, registry passwords, env values from the Project's env file) are replaced by `***` in log text before storage (best effort, exact-match).

### 1.6 Failure modes and UI surfacing
| Failure | Surfaces as |
|---|---|
| Collision | Pre-deploy fail at step 7 and inline on the Ports form with conflicting owner |
| Git auth/host key failure | Failed Deployment; message hints "add the Deploy Key's public key to the repo / check host key policy" |
| Non-fast-forward pull | Failed; offers audited "Reset checkout to origin/<branch>" (`git reset --hard`) |
| Registry auth/pull failure | Failed at step 6 with registry host named; credential values never shown |
| Compose plugin missing | Fails pre-flight; `/api/v1/health` shows `docker compose: missing` |
| Cancel | Status `cancelled` + partial-state notice |
| Daemon restart | `interrupted` with restart notice (recovery) |
| Webhook rejected | 401/202/413 JSON; GitHub shows the response in its delivery log; Audit entry `deployment.trigger` with `outcome=denied` (no body stored) |

### 1.7 Security notes
- **Secrets management**: Deploy Token doubles as GitHub HMAC secret (legacy design) — one secret, two uses; stored encrypted, shown only to `secrets`. Deploy Key private key never in argv/log; temp materialization 0600; Deploy Keys are per Project so a leak is scoped.
- **Input validation**: all paths resolved and containment-checked; refs/URLs validated before use; `--` before positional args; Git protocol allow-list defeats `ext::` command execution.
- **Supply chain / trust**: Deployments run arbitrary compose from the repo as root-equivalent on the Host (ADR 0007). Anyone with `deploy` on a Project can execute code on the Host; state this in the UI and README.
- **Access control**: webhook routes accept bearer/HMAC only; Member permissions `deploy`/`config`/`secrets` gate the respective actions.
- **Availability**: rate limit, body caps, per-step timeouts, log caps.

### 1.8 Acceptance criteria
- [ ] Create Git Project with Deploy Key; public key shown; first Deployment clones via `GIT_SSH_COMMAND` and the private key file is gone afterwards.
- [ ] Create image Project; Deployment pulls and runs generated compose with `-p <slug>`.
- [ ] Compose publishing `0.0.0.0:8080` or bare `8080:80` is rejected; `127.0.0.1:8080:80` accepted and recorded.
- [ ] Collision with another Project, a foreign container, and a host listener are each detected; a redeploy of the same Project does not collide with itself.
- [ ] Two simultaneous triggers: one runs, one queues, a third returns the queued one; never two `running` for a Project.
- [ ] Cancel of a running Deployment terminates the whole child process group within ~5 s and sets `cancelled`.
- [ ] Rollback preview shows commit counts, ≤40 file diffstat, and compose/env diffs; rollback deploys the prior commit (Git) / prior digest (image) with the exact compose and `.env` snapshots of that Deployment (D25).
- [ ] `/deploy` rejects wrong/missing token; GitHub route rejects bad signature, accepts valid push on matching branch, ignores other branches/events, dedupes delivery IDs.
- [ ] SSE stream shows live log, resumes after reconnect without duplicates/gaps, closes on `done`.
- [ ] Yanto restart mid-Deployment yields `interrupted` with notice; queued ones resume.
- [ ] No secret value appears in logs, audit metadata, `ps` output, or error banners.

---

## 2. Containers

### 2.1 Behavior
- **List** all containers (`list_containers(all=true)`), joined with one-shot stats for running ones, and mapped to Projects by label `com.docker.compose.project == projects.slug`. Columns: id, name, image, state/status, published ports, created, CPU %, memory usage/%, compose project/service, owning Project. Sorted newest first. Cache 5 s with a revision counter bumped by any mutation; concurrent readers share one in-flight fetch. CPU % is computed with the standard delta formula from two stats samples (ASSUMPTION on exact bollard stats fields; unverified).
- **Actions**: start, stop, restart (stop timeout 10 s, ASSUMPTION). Authorization: `runtime` on the owning Project; containers belonging to no Project need Owner. **Protected**: the Hosted Registry container (`yanto-registry`) accepts actions only from the Registry page (§4), not here (identified by label `yanto.managed=registry`).
- **Logs**: last 500 lines (`logs(stdout,stderr,tail=500,timestamps)`); live follow via SSE (`follow=true`, `since=<last ts>`), demultiplexed stdout/stderr; TTY containers return raw stream. HTML-escaped on render.
- **Exec terminal** over WebSocket (D13): `GET /containers/{id}/exec` serves the page, `/containers/{id}/exec/ws` upgrades to WS (03 §b).

### 2.2 Exec terminal flow (bollard)
1. Authorize: valid session cookie, `Origin` header equals the configured dashboard origin (blocks cross-site WebSocket hijacking since cookies are sent automatically), permission `runtime`, container `running`.
2. `create_exec(container, { cmd: ["sh","-c","command -v bash >/dev/null && exec bash || exec sh"], attach_stdin/stdout/stderr: true, tty: true, env: ["TERM=xterm-256color"], user: <optional, Owner only> })`.
3. `start_exec(exec_id, { detach: false, tty: true })` → attached `{ output: Stream, input: AsyncWrite }`.
4. Pump: WS binary frames → `input`; `output` chunks → WS binary frames. WS text frames carry JSON control: `{"type":"resize","cols":N,"rows":M}` → bollard exec resize (Context7 documents `resize_exec_tty` with width/height options; exact name and options differ between bollard versions — **verify against the pinned version**). Cap one frame at 64 KiB.
5. Teardown (WS close, idle 15 min, max session 4 h — ASSUMPTION values): close `input` so the TTY hangs up the shell; then `inspect_exec` to log exit code. Docker has no exec-kill API, so lingering processes are possible if a program ignores SIGHUP (**unverified**; mitigation: after teardown, if `inspect_exec.running` stays true for 10 s, log a warning in Audit).
6. Limits: max 3 concurrent sessions per account, 20 total (ASSUMPTION).

### 2.3 Host/Docker side effects
Engine API calls only: list, stats, start, stop, restart, logs, exec create/start/inspect/resize. No files written. Cache invalidated after actions and after every Deployment.

### 2.4 Failure modes
| Failure | UI |
|---|---|
| Docker socket unreachable | Page-level banner "Docker unavailable" (also `/api/v1/health`), actions disabled |
| Container gone between list and action | Toast "container no longer exists", list refresh |
| Exec on stopped container | Terminal refuses before WS upgrade with 409 and message |
| WS Origin mismatch / no permission | 403, no exec created |
| Shell missing (distroless) | Terminal prints "no shell found in image" and closes (exec exits 127) |
| Stats timeout | Row renders without CPU/mem (never blocks the list) |

### 2.5 Security notes
- **Access control / privilege**: an exec shell is root in the container and, with socket-adjacent mounts, potentially on the Host. Gated by `runtime`, audited at start and end (container, account, exec id, duration, bytes in/out); keystrokes and output are **not** recorded (open question: optional session recording).
- **Session security**: WS requires Origin check + session; exec is dashboard-only in v1 (D24): no REST or MCP exec, API Tokens cannot open terminals.
- **Resource limits**: session caps, idle timeout, frame size cap.

### 2.6 Acceptance criteria
- [ ] List shows Project mapping, CPU/mem; a slow `stats` does not delay the list beyond 5 s cache behavior.
- [ ] Start/stop/restart work with `runtime`; denied without; `yanto-registry` refuses from this page.
- [ ] Logs tail 500 and follow live; content is escaped.
- [ ] Terminal: interactive shell, resize works, exit closes WS; Origin mismatch rejected; session start/end in Audit.
- [ ] Idle terminal is closed after the timeout.

---

## 3. System

### 3.1 Behavior
- **Usage**: CPU %, memory, swap, load average, uptime, disks.
  - CPU: background sampler reads `/proc/stat` every 5 s; reports the 5 s and 1 min averages. Memory/swap: `/proc/meminfo` (`MemTotal`, `MemAvailable`, `SwapTotal`, `SwapFree`). Load/uptime: `/proc/loadavg`, `/proc/uptime`. Disk: `statvfs` for `/`, `DATA`, `PROJECTS_ROOT`, and Docker root dir (from `info()`), deduplicated by device.
  - Docker usage: Engine `df` (images, containers, volumes, build cache sizes).
- **Docker prune with preview** (Change: finer-grained than legacy).
  - Preview is read-only and returns candidate lists with sizes: dangling images, unused networks, build cache, stopped containers (each with owning Project, if any). Volumes are never candidates.
  - Execute takes a selection. Defaults on: build cache, dangling images, unused networks. **Off by default**: stopped containers (legacy pruned them unconditionally; here a deliberately stopped Project's containers would be lost). Sequence: build prune, image prune (dangling only), network prune, container prune; each returns reclaimed bytes.
  - Legacy `apt-get clean` is dropped (package management is roadmap).
- **Health**: `GET /healthz` unauthenticated, returns `200 ok` if the process serves requests (liveness only). `GET /api/v1/health` requires `read`: DB `SELECT 1`, Docker `ping` + version, `docker compose version`, disk free for `DATA`, Host Daemon statuses (frps/cloudflared/Traefik/Hosted Registry: installed / active), each with a 3 s deadline and independent result; overall `ok | degraded | down`.
- **App logs**: Owner-only viewer over the in-memory ring buffer of the last 500 `tracing` lines (01 §9) with SSE follow; older history is `journalctl -u yanto` on the Host. Secret redaction is applied at log emission time (tracing layer), not on read.

### 3.2 Host side effects
Reads: `/proc/*`, `statvfs`. Writes: none for usage/health. Prune: Engine API `prune_*` calls only. Cache usage snapshot 5 s.

### 3.3 Failure modes
| Failure | UI |
|---|---|
| Docker down | Usage still renders host stats; Docker panels show error; health `degraded` |
| Prune partially fails | Result table per step (ok/error + message); later steps still run |
| Prune stale preview | Execute re-validates selection and reports items already gone |

### 3.4 Security notes
- **Information disclosure**: unauthenticated surface limited to `/healthz` with no detail; detailed health needs a token.
- **Availability / data loss**: prune never touches volumes; preview-before-execute; typed confirmation not required (non-destructive set) except when stopped containers are selected.
- **Audit**: prune executions audited with reclaimed totals.

### 3.5 Acceptance criteria
- [ ] Usage page shows CPU/mem/swap/load/uptime/disks matching `top`/`df` within sampling tolerance.
- [ ] Prune preview lists candidates and never lists volumes; executing with defaults leaves stopped containers intact.
- [ ] `/healthz` is public and minimal; `/api/v1/health` reports each dependency with its own status and returns within ~3 s when Docker hangs.
- [ ] App logs stream live for the Owner and are denied to Members.

---

## 4. Hosted Registry and Registry Credentials

### 4.1 Behavior
Optional (D10). When enabled, Yanto runs one `registry:2` container (CNCF Distribution) bound to `127.0.0.1:5000`, with htpasswd basic auth and delete enabled. The operator manages registry users, browses repositories/tags, deletes tags, and runs garbage collection from the dashboard. Projects deploy images from it. It is a workload (ADR 0006), so Yanto creates and owns the container, unlike Host Daemons.

Image tag `registry:2` is fixed by D10. Whether it tracks Distribution 2.x or a later major is **unverified** (it affects the in-container config path, §4.2); pin by digest after first pull and record it (ASSUMPTION).

### 4.2 Files and container (exact)
Files, all root-owned:
- `DATA/registry/auth/htpasswd` (0600): one line per `registry_users` row plus the internal user, `name:<bcrypt hash>`; derived from the DB, which is the source of truth for hashes (02 §4.3).
- `DATA/registry/data/` (blobs, bind-mounted at `/var/lib/registry`; survives container recreation).
- `DATA/registry/config.yml` (0644): rendered by Yanto:
  ```yaml
  version: 0.1
  log: { level: info }
  storage:
    filesystem: { rootdirectory: /var/lib/registry }
    delete: { enabled: true }
  http: { addr: :5000 }
  auth:
    htpasswd: { realm: Yanto Registry, path: /auth/htpasswd }
  ```
  Keys `storage.delete.enabled`, `auth.htpasswd.realm/path`, `http.addr`, and `maintenance.readonly.enabled` are grounded in the Distribution configuration docs. `version: 0.1`, `log.level` and `storage.filesystem.rootdirectory` appear in the GC docs/examples; the `log` block shape is **unverified**.

Container (bollard `create_container`/`start`): name `yanto-registry`, image `registry:2`, labels `yanto.managed=registry`, `HostConfig.PortBindings 5000/tcp → 127.0.0.1:5000`, `RestartPolicy unless-stopped`, binds `DATA/registry/config.yml:/etc/docker/registry/config.yml:ro` (**unverified** path; newer Distribution images may read `/etc/distribution/config.yml` — verify against the pulled image's `Cmd`/docs before implementing, or pass the path explicitly as the container command), `DATA/registry/auth:/auth:ro`, `DATA/registry/data:/var/lib/registry`. Disable removes the container only; data and files are kept unless "delete data" is chosen with typed confirmation.

htpasswd facts (Distribution docs): the only supported format is **bcrypt**; entries with other hash types are **ignored**; the file is **loaded once at startup**; with TLS absent basic auth sends passwords in cleartext headers; a missing file is auto-created with a generated user (so Yanto must always write the file *before* first start). Therefore:
- Hash with bcrypt (cost 10, ASSUMPTION), `$2y$` prefix as `htpasswd -B` produces (ASSUMPTION that Go bcrypt accepts `$2y$`; Apache-compatible files rely on it — test in CI).
- Any user add/remove/password change = rewrite the file atomically (temp file in `DATA/registry/`, fsync, rename) **then restart the container** so it reloads (no hot reload documented).
- Passwords are shown once at creation, stored only as hashes. Min length 16 for generated, 12 for chosen (ASSUMPTION). Username `^[a-z0-9._-]{1,32}$`.
- Yanto keeps one internal user `yanto-internal` whose password is stored encrypted in `hosted_registry_settings.internal_password_enc` (it must be recoverable to browse via the API and to pull for Projects); its bcrypt line is added when the file is rendered and it is not a `registry_users` row.

### 4.3 Browse, delete, GC
**Browse** (Yanto calls `http://127.0.0.1:5000` with `yanto-internal` basic auth):
- Repositories: `GET /v2/_catalog?n=<n>` and follow `last=<value>` pagination until no `Link: …rel="next"` header (pagination defined for tags; Distribution docs state catalog behaves identically).
- Tags: `GET /v2/<name>/tags/list?n=<n>&last=<tag>` (default 100 per page, `Link` header for next).
- Per tag: `HEAD /v2/<name>/manifests/<tag>` with `Accept: application/vnd.docker.distribution.manifest.v2+json` (plus OCI manifest/index media types, ASSUMPTION) to read `Docker-Content-Digest`; manifest `GET` for size/layers/created is best effort. Multi-arch indexes show as one row with child count.
- Registry unreachable/401 → page shows state "registry down / auth mismatch" with container status from Docker.

**Delete tag**: the API deletes **by digest only** (`DELETE /v2/<name>/manifests/<digest>` → `202 Accepted`, `404` if already gone). Flow: resolve tag → digest, list all other tags pointing to the same digest, show them in the confirmation ("deleting this will also remove tags: …"), then delete. Requires `storage.delete.enabled: true`. Blobs remain on disk until GC.

**Garbage collection** (Distribution docs: `registry garbage-collect [--dry-run] [--delete-untagged] [--quiet] /path/to/config.yml`; docs say to restart the registry with `maintenance.readonly.enabled: true` before GC, and restart again without it afterwards):
1. Preview: `docker exec yanto-registry registry garbage-collect --dry-run [--delete-untagged] <config path>` (exec via bollard); show the output.
2. Run: render `config.yml` with `maintenance: { readonly: { enabled: true } }`, restart the container (pushes now rejected, pulls still work), `exec ... registry garbage-collect [--delete-untagged] <config path>`, stream output to an SSE log, then re-render the normal config and restart. A drop-guard restores normal mode even on error; on Yanto restart a persisted `registry_gc_in_progress` flag triggers the same restore.
3. Record bytes reclaimed from `df` of the volume before/after (via a throwaway helper container `du`; best effort).
- Alternative considered: stop the container and run `registry garbage-collect` in a one-off container on the same volume. Rejected as default because pulls would also fail during GC; kept as documented fallback if readonly mode misbehaves (**unverified** under load).
- Concurrency: GC, user changes, and enable/disable are mutually exclusive (one registry-op mutex).
- Warning in UI: untagged manifests (e.g. old digests referenced by rollback targets, §1.1) are deleted by `--delete-untagged`; default off.

### 4.4 How Projects reference it
- Image reference `127.0.0.1:5000/<repo>:<tag>` (or `<digest>`). Pulling from `127.0.0.1` works over plain HTTP because Docker treats loopback registries as insecure by default (**unverified** here; confirm in Docker docs). The Docker daemon, not the container network, resolves this, so it works for every Project.
- Credentials for pulls are the internal user, injected via the per-command `DOCKER_CONFIG` (§4.7) when a Project's image host is `127.0.0.1:5000`; no operator action.
- Compose files inside Git Projects can use the same reference; Yanto detects the host and injects auth likewise.
- A Project referencing the Hosted Registry while it is disabled/down fails at the pull step with a hint to enable it.

### 4.5 Exposing for remote push
Docker clients require HTTPS for non-loopback registries, so a Hostname is mandatory. Two supported paths, both created as ordinary Hostnames (§9) pointing at `http://127.0.0.1:5000`; the registry is not a Project, so a Hostname may have `target_kind='registry'` (02 §4.6; Owner-only; glossary note: OQ-9):
- **Cloudflare Tunnel**: easiest (no open ports, edge TLS). **Caveat**: Cloudflare limits request body size on proxied traffic, and image layer uploads (`PATCH`/`PUT` of blobs) can exceed it. The limit depends on plan and is **unverified** here; check Cloudflare's current limits docs before documenting a number. Layers above the limit fail to push with a connection/413-style error. Mitigations: keep layers small, push from the Host itself (loopback), or use Traefik/FRP.
- **Traefik** (ports 80/443, Let's Encrypt): no body limit imposed by a CDN; DNS record should be DNS-only (not proxied) (§9). Recommended for large images.
- **FRP** (frps only): an FRP Tunnel can expose a port but terminates no TLS; pushing needs TLS to the registry, so use FRP only when TLS is handled elsewhere (ASSUMPTION; mostly not recommended). Yanto does not wire it; docs describe it.
- Basic auth always on; the UI warns when a remote Hostname is not HTTPS-terminated.

### 4.6 Registry users, UI, and state
- State: `hosted_registry_settings(enabled, listen_port=5000, container_name, image_digest, gc_in_progress, internal_password_enc, …)` and `registry_users(username, password_hash, …)` (02 §4.3). The DB is the source of truth; the htpasswd file is regenerated from it.
- Permissions: Owner (`admin` scope) only for everything on the registry page: enable/disable, users, repo/tag browse, delete, GC (03 §a.2). Members have no registry access.

### 4.7 Registry Credentials (external registries)
- Entity: `registry_credentials(id, name, registry_host, username, secret_enc)`; `registry_host` normalized (`ghcr.io`, `docker.io`→ key `https://index.docker.io/v1/`, `host:port`). A Project references at most one credential (`projects.registry_credential_id`); at deploy only that credential is used, and only for the hosts it matches. Validate on save with a login probe: `GET https://<host>/v2/` and follow the `WWW-Authenticate` challenge (ASSUMPTION; failures reported but not blocking).
- **Auth is passed per command via a temp `DOCKER_CONFIG` directory**, not by writing `/root/.docker/config.json` and not via `docker login`:
  - Per Deployment (and per prune/pull action that needs auth) create `RUN/docker-<id>/` (mode 0700, on tmpfs so it never hits disk), write `config.json` containing only `{"auths": {"<host>": {"auth": "<base64(user:secret)>"}}}` (mode 0600), run `docker pull` / `docker compose pull|up|build` with `DOCKER_CONFIG` pointing there, then delete it in a drop-guard; startup sweeps stale `RUN/docker-*`.
  - Justification: (1) **secrets management**: the secret exists on tmpfs only for the command's lifetime instead of persisting in plaintext base64 in root's home, which any later root process or backup would pick up; (2) **isolation**: Project A's pull cannot use Project B's credential, and manual `docker` use on the Host is unaffected; (3) **no global mutation**: concurrent Deployments cannot clobber each other's login state, which `docker login`/`logout` on a shared file would; (4) **auditability**: credential use is tied to a Deployment id.
  - Trade-offs/risks: `config.json` still holds a base64 (reversible) secret during the command; mitigated by 0700 dir on tmpfs. Changing `DOCKER_CONFIG` also changes where the CLI looks for **plugins** (`cli-plugins`) and `buildx` state — if `docker compose` lives under root's `~/.docker/cli-plugins`, it would disappear (**unverified**; mitigated by symlinking `RUN/docker-<id>/cli-plugins` to the real plugin dir when it exists, and by the pre-flight `docker compose version` run under the same env). Credential helpers (`credsStore`) are intentionally not used.
  - `docker login` is never run; no secret is passed on argv (`--password` would leak via `ps`).
- Selection: for each image/compose service the registry host is derived from the reference (first component containing `.` or `:`, or `localhost`; otherwise Docker Hub). Only the Project's credential, and only if its host matches, is written.
- Reveal: secret never revealed after creation (rotate only); `secrets` permission needed to create/rotate.

### 4.8 Failure modes
| Failure | Surfaces as |
|---|---|
| Port 5000 already in use at enable | Enable fails with owner of the listener (same host-listener scan as §1) |
| htpasswd invalid (e.g. non-bcrypt line) | Registry exits at start; Yanto reads container exit + last log lines and shows them; the file is re-rendered from `registry_users` (offer "re-render htpasswd" in the UI) |
| Auth mismatch after user changes without restart | Browse shows 401 banner with "restart registry" action |
| Push via Cloudflare fails on big layers | Documented caveat; Diagnostics note on the Hostname |
| GC interrupted | Readonly flag recovered on restart; banner "Registry was left read-only; restore" |
| Delete when delete disabled (hand-edited config) | `405`/error shown; config re-render action |

### 4.9 Security notes
- **Access control**: loopback bind + basic auth + HTTPS-only remote exposure; users are least-privilege-ish — htpasswd has no per-repo ACL, so every user can push/pull/delete everything (state in UI; token auth would need a separate service — out of scope).
- **Secrets management**: bcrypt hashes only; internal credential encrypted; DOCKER_CONFIG on tmpfs.
- **Transport security**: refuse to show remote-push instructions unless a TLS Hostname exists.
- **Supply chain**: image pinned by digest; the registry content is operator-controlled, but any registry user can overwrite tags that a Project deploys — advise digest references.
- **Availability**: GC window is write-unavailable by design.

### 4.10 Acceptance criteria
- [ ] Enabling creates config, htpasswd (with `yanto-internal`), data dir, and a container bound only to `127.0.0.1:5000`; `curl` to `/v2/` without auth returns 401.
- [ ] Adding a registry user rewrites the htpasswd atomically, restarts the container, and `docker login` with it succeeds; removing it denies access.
- [ ] Catalog and tag lists page correctly beyond 100 tags.
- [ ] Deleting a tag lists sibling tags first, deletes by digest, returns 202; blobs reclaimed only after GC.
- [ ] GC dry-run output shown; real GC runs with pushes rejected and restores normal mode afterwards even if Yanto restarts mid-GC.
- [ ] A Project with `127.0.0.1:5000/app:tag` deploys without any manual login.
- [ ] Private GHCR image deploys using a Registry Credential; `config.json` exists only during the command; no `~/.docker/config.json` is created or modified.
- [ ] `docker compose` still works under the temp `DOCKER_CONFIG` on a Host with plugins under `~/.docker/cli-plugins`.
- [ ] Cloudflare size-limit caveat appears in the UI near Hostname creation for the registry, marked "limit not verified".

---

## 5. Cron Entries

### 5.1 Behavior
Yanto owns exactly one cron file, `/etc/cron.d/yanto` (D11). Entries are stored in SQLite (source of truth) and the whole file is regenerated and atomically replaced on every change. Other cron sources are shown **read-only**. Run history is provided by a wrapper (decision below).

### 5.2 `/etc/cron.d` format rules (from cron(8)/crontab(5) as implemented by Debian cron; **unverified** in this session — check against the target distros, and against cronie on RHEL-like Hosts if ever supported)
- Line: `m h dom mon dow user command`. The **user field is required** in `/etc/cron.d` (unlike per-user crontabs) and must be an existing account.
- Environment lines `NAME=value` are allowed at the top; Yanto writes `SHELL=/bin/sh`, `PATH=/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin`, and optionally `MAILTO=""` (setting `MAILTO` empty disables mail; configurable). No variable expansion between lines.
- **Filename**: Debian ignores files in `/etc/cron.d` whose names contain characters other than letters, digits, underscore, hyphen (in particular **no dots**, so `yanto` is valid but `yanto.cron` and `yanto.bak` are silently ignored). Therefore any temp/backup file name must contain a dot.
- **Ownership/mode**: owned by root, not writable by group/other; Yanto writes `0644 root:root`. Not a symlink (refuse if the existing path is a symlink).
- **Trailing newline**: the last line must end with `\n` or cron may ignore it.
- `%` in the command field is converted to newline by cron; literal `%` must be `\%`. No line continuations (`\` + newline is not supported). Commands are single-line.
- Schedules: five fields or one of `@reboot @hourly @daily @weekly @monthly @yearly @annually @midnight` (macro support is **unverified** per cron implementation; Yanto allows `@reboot/@hourly/@daily/@weekly/@monthly/@yearly`).

### 5.3 Entries, validation, and the generated file
Entry (02 §4.4 `cron_entries`): `{id, name, schedule, run_as_user, command, enabled, overlap (allow|skip), timeout_s?}`; runs in `cron_runs` (D26). Every entry is wrapped; there is no unrecorded form.
- **Validation of schedule**: Yanto parses with its own validator (ranges: min 0–59, hour 0–23, dom 1–31, mon 1–12 or `jan–dec`, dow 0–7 or `sun–sat`; lists `,`, ranges `-`, steps `/`, `*`); rejects empty fields, steps of 0, descending ranges, and wrong field count; shows "next 5 run times" (computed in the Host's timezone; note cron's dom/dow OR semantics when both are restricted). `run_as_user` must resolve via `getpwnam`; the UI shows a dropdown of login-capable users plus `root`. `command` must be non-empty, single-line (no `\n`/`\r`), no NUL, ≤ 4096 bytes.
- **Generated file** (header comment, "managed by Yanto, edits are overwritten"):
  - Every enabled entry: `<schedule> root /usr/local/bin/yanto cron-run <id>` (binary path = `current_exe`). The user field is `root` on purpose; the entry's Unix user is `run_as_user` (see below and OQ-9).
  - Disabled entries are emitted as `# disabled: <id>` comments, or omitted (comment form; ASSUMPTION so the file documents state).
- `yanto cron-run <id>` is invoked by cron as **root** (that is why the user field is `root`), loads the entry from SQLite (opens the DB directly; WAL + busy timeout), then drops privileges for the child: `initgroups` + `setgid` + `setuid` to `run_as_user`, `HOME`, `LOGNAME`, `USER`, `SHELL`, `PATH` set as cron would, then runs `/bin/sh -c <command>`. Because the command is read from the DB, the cron file never contains user-controlled text, which removes `%`-escaping and injection issues entirely.
- **Run history (D26)**: cron discards output unless wrapped (mail disabled), so Yanto records runs through `cron-run`: `cron_runs` (02 §4.4: `status`, `exit_code`, `started_at`, `finished_at`, `output_tail`); output captured combined, tail 64 KiB, UTF-8 lossy. Retention: last 50 runs and 30 days per entry (ASSUMPTION). `overlap=skip` uses `flock` on `RUN/cron-<id>.lock` and records a `skipped` run. `timeout_s` → SIGTERM then SIGKILL after 5 s. If Yanto's DB is unreadable, `cron-run` still executes the command? **No** — it exits 75 and logs to syslog (journal) since it cannot know the command (fail closed).
- Rejected alternatives: keeping the user field as the real user and having `cron-run` talk to the daemon over a socket (more moving parts, needs a socket auth story); wrapping with `logger` (no structured history).

### 5.4 Host side effects (exact)
1. Render to a string; verify it ends with `\n`.
2. Write to `/etc/.yanto-cron.tmp` (a dotted name, on the same filesystem, so cron never reads it), mode `0644`, owner root; `fsync`; `rename` over `/etc/cron.d/yanto`; `fsync` `/etc/cron.d`. Cron daemons re-scan on mtime change (Debian cron checks `/etc/cron.d` each minute; **unverified**), so no reload command is run.
3. Store a SHA-256 of the rendered content in `settings`; on page load, compare with the file on disk: mismatch → banner "file edited outside Yanto" with "re-apply" (overwrite) — never silently merged.
4. Read-only view (no writes): `/etc/crontab`, other regular files in `/etc/cron.d/` (excluding `yanto`), names in `/etc/cron.{hourly,daily,weekly,monthly}/`, and per-user spools (`/var/spool/cron/crontabs/*` on Debian, `/var/spool/cron/*` elsewhere; **ASSUMPTION** for paths). Raw text shown, 256 KiB cap per file, non-UTF-8 shown as "binary/unreadable". Never edited.

### 5.5 Failure modes
| Failure | UI |
|---|---|
| Invalid schedule/user/command | Form validation error at the field; nothing written |
| Write fails (read-only FS, ENOSPC) | Entry save rolls back (DB transaction), banner with errno |
| `/etc/cron.d/yanto` is a symlink or wrong owner | Refuse with explanation; offer "replace" only for Owner |
| Cron daemon not installed/running (`systemctl is-active cron`/`crond`) | Page warning "cron service inactive: entries will not run" (check only) |
| External edit detected | Drift banner (above) |
| Entry's `run_as_user` deleted from Host | Run recorded `failed` (`unknown user`); entry flagged invalid |
| `cron-run` failure to open DB | Exit 75, journal line, run not recorded |

### 5.6 Security notes
- **Privilege / access control**: entries run commands as any chosen user including root, from a root-run service: creating/editing needs a dedicated permission (`admin` scope / Owner; Members cannot, ASSUMPTION). Audit logs the full command text (no secrets expected; UI warns not to put secrets in commands).
- **Input validation**: single-line commands, NUL/newline rejection; the wrapper form keeps user text out of the cron file.
- **Integrity**: atomic replace, perms checked each write, symlink refusal.
- **Privilege drop correctness**: `initgroups`, `setgid` before `setuid`; refuse if the target UID resolves to 0 while the entry user is not `root`.

### 5.7 Acceptance criteria
- [ ] Creating an entry produces `/etc/cron.d/yanto` with mode 0644 root, trailing newline, valid environment header; cron runs it within a minute.
- [ ] Invalid schedules (`61 * * * *`, `*/0 * * * *`, 4 fields) are rejected with field-level errors; the next-5-runs preview matches `cron` semantics on tested cases.
- [ ] Temp file during write has a dotted name and is never executed by cron.
- [ ] Recorded run stores exit code, duration, and output tail; `overlap=skip` prevents concurrent runs; timeouts kill the process group.
- [ ] A command containing `%` runs correctly.
- [ ] Editing the file by hand triggers the drift banner; "re-apply" restores it.
- [ ] Other cron files are visible but have no edit controls and are never written.

---

## 6. Authorized Keys

### 6.1 Behavior
Yanto manages Authorized Keys for **one configured Unix user** (`config.toml` `[ssh] authorized_keys_user`, chosen by `yanto install`; no built-in default, 05 §3.2; D12) inside a marked block of that user's `~/.ssh/authorized_keys`. Everything outside the block is opaque and preserved byte-for-byte.

Block markers (exact lines):
```
# BEGIN YANTO MANAGED KEYS (do not edit between these lines)
...one key per line...
# END YANTO MANAGED KEYS
```

### 6.2 Inputs / state
- `authorized_keys(id, name, key_type, public_key, fingerprint, enabled, created_at)` (02 §4.4); SQLite is the source of truth for the block's content (disabled rows are omitted from the block).
- Home dir from `getpwnam(user)`, never from `$HOME`.

### 6.3 Key parsing and validation
- Input: one OpenSSH public key line `type base64 [comment]`. Reject: multiple lines, leading **options** (`command=`, `from=`, etc. — options can change what a key may execute; not supported in v1, ASSUMPTION), private key material (`-----BEGIN`), control characters, >8 KiB.
- Accepted types: `ssh-ed25519`, `sk-ssh-ed25519@openssh.com`, `ecdsa-sha2-nistp256|384|521`, `sk-ecdsa-sha2-nistp256@openssh.com`, `ssh-rsa` (RSA ≥ 3072 bits accepted, 2048–3071 accepted with warning, <2048 rejected — ASSUMPTION policy). `ssh-dss` rejected.
- Decode base64, check the embedded type string equals the declared type, check blob length sanity; compute `SHA256:<base64>` fingerprint (as `ssh-keygen -lf`). Implementation may use a Rust SSH-key parsing crate or `ssh-keygen -l -f -` (decision for implementation; both acceptable).
- The stored line is rebuilt canonically: `type base64 yanto:<id>:<name>` where the name is sanitized to `[A-Za-z0-9._@ -]{1,64}` (the user's original comment is kept only as the name).
- Duplicate fingerprint → rejected ("already present", also when present outside the block, shown as a warning rather than blocking).

### 6.4 Host side effects (exact)
1. Resolve user and home; refuse if the user does not exist or has a non-absolute home.
2. Ensure `~/.ssh` exists (create `0700`, chown user); if it exists with group/other write, refuse (sshd StrictModes would reject the file — ASSUMPTION about `StrictModes`; `sshd -T` reveals it).
3. Read `authorized_keys` (if absent, treat as empty). If it is a symlink, refuse (ASSUMPTION; avoids writing through to unexpected targets). Record `(size, mtime, sha256)`.
4. Parse into lines. Find marker lines: zero blocks → append (ensuring the prior content ends with `\n`, then the block); one well-formed block → replace the lines strictly between the markers; **anything else** (BEGIN without END, END before BEGIN, two blocks) → refuse with "ambiguous markers; fix manually" and write nothing.
5. Write temp file `~/.ssh/.authorized_keys.yanto.tmp` (same dir, mode 0600, chown user:primary-group), `fsync`; re-stat the original — if `(size, mtime, sha256)` changed since step 3, abort and retry once (optimistic concurrency against manual edits); copy previous content to `~/.ssh/authorized_keys.yanto.bak` (0600) once per write; `rename` over `authorized_keys`; `fsync` dir.
6. Final perms: file `0600` owned by the user. Directory unchanged except creation.
7. Verification after write: re-read, confirm outside-block content hash equals the pre-write hash and the block equals the expected lines. If not, restore from `.bak` and fail loudly.
8. Informational check (no writes): `sshd -T` (if available) to read `authorizedkeysfile`, `pubkeyauthentication`, and `strictmodes`; warn if effective `AuthorizedKeysFile` does not include `.ssh/authorized_keys` or pubkey auth is disabled (the managed keys would then have no effect).

### 6.5 Lockout prevention
Goal: Yanto never removes the operator's last way in.
- **Rule 1 (hard)**: a change is refused if, after applying it, the target file would contain **zero** valid key lines in total (inside the block plus outside). Message names the rule. Override only by Owner with typed confirmation of the Unix username, audited as `authorized_key.force_change`.
- **Rule 2**: never edits or removes lines outside the block, including keys the operator added by hand (so those stay as an independent way in).
- **Rule 3**: `.bak` of the previous file kept; the UI shows the path and a one-line restore command.
- **Rule 4**: removing a key shows its fingerprint and label and the count of remaining keys; deleting the *only* Yanto-managed key when outside keys exist is allowed.
- **Rule 5** (ASSUMPTION): Yanto does not touch `sshd_config`, does not restart `sshd`, and does not manage password authentication.
- It cannot know which key the current SSH session uses; hence the count-based rule instead of identity-based.

### 6.6 Failure modes
| Failure | UI |
|---|---|
| Unparseable/unsupported key | Field error naming the reason (type, options, length) |
| Ambiguous markers | Banner, read-only mode for the block until fixed |
| External change during write | Retry once, then error "authorized_keys changed, reload and retry" |
| Verification mismatch | Auto-restore from `.bak`, error with diff summary |
| User missing / home missing | Settings page error; feature disabled |
| Lockout rule violation | Refusal message (Rule 1) |

### 6.7 Security notes
- **Access control**: Authorized Keys grant Host shell access (as the configured user, likely root); gate on Owner/`admin` only. Audit add/remove with fingerprint and label (not the key body is fine; fingerprint suffices).
- **Input validation**: single-line canonical rebuild defeats option injection and newline smuggling (a key comment containing `\n` could otherwise add a second line).
- **Integrity / availability**: atomic replace, verification, lockout rules.
- **File permissions**: 0600/0700 enforcement; sshd StrictModes interplay.

### 6.8 Acceptance criteria
- [ ] Adding a valid ed25519 key creates the block with markers when absent; file is `0600`, owned by the user; SSH login works.
- [ ] Content outside the block is byte-identical before/after any add/remove (hash test), including files without a trailing newline and with CRLF lines.
- [ ] Keys with options, multiple lines, DSA, short RSA, or private key text are rejected with specific errors.
- [ ] Ambiguous markers cause refusal with no write.
- [ ] Removing the last key when no keys exist outside the block is refused; with outside keys it succeeds.
- [ ] A concurrent manual edit during a write is detected and not clobbered.
- [ ] `.bak` is written; verification failure restores it.

---

## 7. Backups

### 7.1 Behavior (D14)
Backups are **local only** and **downloadable** (R2 dropped). Four kinds:
1. **Volume archive**: all named Docker volumes of a Project (or selected ones) as `.tar.zst`.
2. **Postgres dump** and 3. **MySQL/MariaDB dump** from the Project's declared database service.
4. **Yanto SQLite**: online snapshot of Yanto's own database.

Storage: `DATA/backups/` (0700), files 0600, laid out as in 01 §8.2: `projects/<slug>/<UTC ts>-<kind>.<ext>` and `yanto/<UTC ts>-yanto.tar.zst`; free-space guard before start (refuse if free < max(1 GiB, 2× the previous backup of the same series) — ASSUMPTION). Each backup row stores size and SHA-256 (shown for download verification). Manual backups are never auto-deleted.

### 7.2 Inputs / state
- `backups` (02 §4.4): `kind` `project_volumes|project_postgres|project_mysql|yanto_sqlite` (CLI aliases `volumes|postgres|mysql|yanto`), `trigger` `manual|api|cron|pre_restore`, `status`, `filename`, `file_path`, `size_bytes`, `sha256`, timestamps, `download_count`.
- A Project **declares** DB services in `project_databases` (02 §4.2: `engine`, `compose_service`, optional `database`, optional `user_env`/`password_env` variable names; engine defaults apply when null; D14). Container = the one with labels `com.docker.compose.project=<slug>` and `com.docker.compose.service=<compose_service>`.
- Helper image setting `backup.helper_image` (default `busybox:stable`, ASSUMPTION; needs only `tar`, `sh`).

### 7.3 Side effects (exact)
**Volume archive**
- Enumerate volumes: Engine `list_volumes` filtered by label `com.docker.compose.project=<slug>` (bind mounts are not backed up; the UI says so).
- Optional "stop Project during backup" (default off; warning that unstopped volumes are crash-consistent only; prefer DB dumps for databases): `docker compose -p <slug> stop` before, `start` after (guaranteed by drop-guard).
- For each volume: create a throwaway container from the helper image: `Cmd: ["tar","-C","/data","-cf","-","."]`, `HostConfig.Binds: ["<volume>:/data:ro"]`, `AutoRemove`, no network (`NetworkMode: none`), labels `yanto.managed=backup-helper`. Attach logs; Yanto reads stdout (the tar stream) and compresses it with zstd in-process, writing to a temp file `DATA/backups/.<name>.partial`; stderr is collected for errors. Archive layout (decision): one `.tar.zst` per volume, grouped under a Backup Set id in the UI.
- Success: container exit code 0, then `rename` partial → final, compute SHA-256 while writing. Failure: delete partial, mark `failed` with the stderr tail.
- Compression level 3 default (ASSUMPTION), configurable.

**Postgres**
- `exec` into the DB container via bollard: `sh -c 'PGPASSWORD="$POSTGRES_PASSWORD" exec pg_dump -U "${POSTGRES_USER:-postgres}" -d "${db}" --clean --if-exists --no-owner'` where `${db}` is the declared database or `$POSTGRES_DB`. Credentials come from the container's own environment, so no secret is read into Yanto or placed in argv. Output (plain SQL) streamed to a zstd writer → `<owner>-postgres-<ts>.sql.zst`. Exit code via `inspect_exec`; non-zero → failed.
- Requires the container `running`.

**MySQL/MariaDB**
- `exec`: `sh -c 'MYSQL_PWD="${MYSQL_ROOT_PASSWORD:-$MARIADB_ROOT_PASSWORD}" exec "$(command -v mariadb-dump || command -v mysqldump)" -u root --single-transaction --routines --triggers --databases "$db"'`; DB name from declaration or `$MYSQL_DATABASE`/`$MARIADB_DATABASE`. Binary names and env var names are image-dependent (**unverified** across images); failures surface the exec stderr. `.sql.zst`.

**Yanto SQLite**
- `VACUUM INTO 'DATA/backups/yanto/.<ts>.db.partial'` through the live connection (consistent snapshot of a WAL database, requires SQLite ≥ 3.27; the alternative is the SQLite online backup API — either satisfies "online backup", implementation choice). Then `PRAGMA integrity_check` on the copy, then tar + zstd the copy together with `keys/app.key` (D22) → `DATA/backups/yanto/<ts>-yanto.tar.zst`, delete the intermediate. File is 0600.
- Content class: **RESTRICTED** once bundled. The archive contains the DB (password hashes, encrypted secrets, audit log) **and** `keys/app.key` (D22), so it alone decrypts every secret. It is downloadable by the Owner only, flagged sensitive in the UI, and written `0600`. Layout: `yanto-<ts>.tar.zst` holding `yanto.db` and `app.key`.

### 7.4 Retention
- Schedules are Cron Entries (§5) running `yanto backup …`; retention policy is a flag, not a separate scheduler: `--keep-last N` (default 7) and/or `--keep-days D`. After a **successful** scheduled run, Yanto deletes older `trigger='cron'` backups of the same (project, kind, target) series beyond the policy and removes their files. Manual and `pre_restore` backups are exempt (pre_restore: keep last 3 per target). Series rows are those with `trigger='cron'`.
- Failed backups older than 7 days are purged. Deleting a `running` backup is refused. `downloaded_at/count` kept.
- Space alarm: `/api/v1/health` degrades when backups dir free space < 10% (ASSUMPTION).

### 7.5 CLI for cron (`yanto backup`)
- `yanto backup yanto [--keep-last N] [--keep-days D]`
- `yanto backup project <slug> (--volumes | --db <service>) [--stop-project] [--keep-last N] [--keep-days D]`
- Runs in-process as root against the same `DATA` dir and Docker socket (does not require the daemon to be up, except for DB/audit writes it opens SQLite directly). Takes an advisory lock `RUN/backup-<series>.lock` (`flock`); a second invocation exits 75 ("busy"). Exit codes: 0 success, 1 failure, 75 busy. Writes an Audit entry (actor `cli`, source `cron` if `YANTO_CRON_RUN` is set by `cron-run`). Stdout one-line summary; verbose with `-v`.
- Intended Cron Entry command: `yanto backup yanto --keep-last 14` (recorded via `cron-run`, so history includes output and exit code).

### 7.6 Restore flow
Restores are explicit, confirmed, and always take a `pre-restore` backup of the current state first (same series).
- **Volume**: confirm by typing the Project name. Steps: pre-restore archive of the target volume; `docker compose -p <slug> stop`; throwaway helper container with the volume mounted rw, `Cmd: sh -c 'find /data -mindepth 1 -delete; tar -C /data -xf -'` fed the decompressed stream on stdin (attached stdin through bollard); verify exit code; `docker compose -p <slug> start`. Option "restore into a new volume `<vol>-restore-<ts>`" (no stop, no deletion) is offered first and is the default.
- **Postgres/MySQL**: container must be running; pre-restore dump; decompress and stream into `psql -U … -d … -v ON_ERROR_STOP=1 --single-transaction` or `mysql -u root` via exec stdin (the dumps contain `DROP … IF EXISTS`/`--clean`). Failure leaves the pre-restore dump available.
- **Yanto SQLite**: **not** done from the UI (the DB is open). CLI only: `systemctl stop yanto && yanto restore yanto <archive> && systemctl start yanto`; the command checks the daemon is stopped (lock file / `systemctl is-active`), moves the current DB aside as `yanto.db.pre-restore-<ts>`, writes the restored file, runs `integrity_check`. Sessions and API Tokens in the restored DB reflect the backup time.
- Restore from an uploaded file is out of scope for v1 (ASSUMPTION): only Yanto-created backups (checksum verified before use).

### 7.7 Download
`GET /backups/{id}/download` streams the file (`Content-Disposition: attachment`, `Content-Length`, `X-Content-SHA256`), range requests supported (ASSUMPTION), `backups` permission on the Project (Owner only for the Yanto-DB archive, which contains `app.key`, D22; REST needs `admin` scope), increments counters, writes Audit entry. No signed public links.

### 7.8 Failure modes
| Failure | UI |
|---|---|
| DB container stopped | Backup refused "service must be running"; scheduled run exits 1 |
| `pg_dump` non-zero (auth, version) | Row `failed` with exec stderr tail; partial deleted |
| Helper image missing/offline | Pull attempted once; failure shows registry error |
| Disk full / low space | Pre-check refusal; mid-run ENOSPC → `failed`, partial removed |
| Yanto crash mid-backup | Startup marks `running` rows `failed` ("interrupted"), removes `.partial` files |
| Restore tar/exit error | Restore aborts, pre-restore backup retained, Project left stopped with banner |
| Concurrent runs | Second run refused (lock) |

### 7.9 Security notes
- **Data protection**: backup files contain customer data and (for Yanto DB) secret material; 0700/0600, downloads authenticated and audited, no remote copy in v1; UI warns that downloaded files should be encrypted at rest by the operator (age/gpg not provided by Yanto).
- **Secrets management**: DB passwords come from the container environment inside the container only; never logged. Helper containers have no network and mount volumes read-only for backups.
- **Input validation**: service/database names validated `^[A-Za-z0-9._-]+$` before interpolation into `sh -c` strings (database name is passed as a positional/env value rather than interpolated where possible — implement via `Env` of the exec, e.g. `YANTO_DB=<name>`).
- **Availability**: space guard, locks, zstd level cap.
- **Integrity**: SHA-256 recorded and verified before restore.

### 7.10 Acceptance criteria
- [ ] Volume backup of a Project with two named volumes yields two `.tar.zst` files with correct contents; the Project keeps running.
- [ ] Postgres and MySQL backups succeed from containers whose credentials exist only in container env; no password appears in `ps` on the Host.
- [ ] Yanto SQLite backup is consistent under concurrent writes and passes `integrity_check`.
- [ ] `yanto backup yanto --keep-last 2` run three times leaves two scheduled backups; manual ones untouched; a second concurrent run exits 75.
- [ ] Restore of a volume into a new volume leaves the original unchanged; in-place restore takes a pre-restore backup and restarts the Project.
- [ ] Restore of a SQL dump into an empty DB succeeds; a corrupted file fails the checksum before any change.
- [ ] Download works with `backups` permission, is audited, and shows the SHA-256.
- [ ] Interrupted backups are cleaned on restart.

---

## 8. FRP (frps only)

### 8.1 Behavior
Yanto drives the Host's `frps` (ADR 0006, D15, D17): status check, config read/write of a defined field subset, proxy listing through frps's dashboard API, and restart via systemd. It never installs `frps`, never writes `frpc` configs, and never creates proxies (clients register them).

### 8.2 Inputs / state
Settings (`config.toml`, 01 §8.1): `[frp] unit` (default `frps`) and `config_path` (auto-detected from `ExecStart`, overridable); the binary is taken from `ExecStart`. No FRP secrets are duplicated in SQLite: the dashboard credentials and auth token are read from `frps.toml` at call time (single source of truth).

### 8.3 Check (read-only)
- Installed: `systemctl show <unit> -p LoadState,ActiveState,SubState,FragmentPath,MainPID,ExecStart --no-pager`; `LoadState=not-found` → "not installed" with operator instructions (install frp from its official releases, create the unit; Yanto shows a sample unit but never writes it).
- Version: run the binary from `ExecStart` with `--version` (flag existence **unverified**; fall back to the dashboard serverinfo response).
- Config path: parse `-c <path>` / `--config <path>` / `--config=<path>` from `ExecStart`; else `/etc/frp/frps.toml` (**ASSUMPTION** default).
- Statuses: not installed / installed-inactive / running / running-without-dashboard (no `webServer.port` in config) / dashboard-unreachable.

### 8.4 Config read/write
- Format: TOML (frp's current format; `frps.toml` with `bindPort`, `webServer.*`, `auth.*`, `allowPorts` confirmed in frp docs via Context7).
- **Fields Yanto owns** (edited via the form; all other keys, tables, and comments are preserved by a comment-preserving TOML editor, e.g. `toml_edit` — crate choice is an implementation decision):
  | Key | Notes |
  |---|---|
  | `bindAddr`, `bindPort` | port 1–65535; must not collide with Host listeners (other than frps itself) |
  | `auth.method`, `auth.token` | Yanto may generate a token (32 random bytes, hex); shown to the Owner on demand (audited) since `frpc` clients need it; stored only in the file |
  | `webServer.addr`, `webServer.port`, `webServer.user`, `webServer.password` | docs: dashboard enabled by setting `webServer.port`; default addr `127.0.0.1`; user/password optional. Yanto defaults to `127.0.0.1` and **requires** user+password when it enables the dashboard (ASSUMPTION policy) |
  | `allowPorts` | list of `{start,end}` / `{single}` entries (documented shape) restricting remote ports |
- Not owned (shown read-only as raw TOML): `vhostHTTPPort`, `vhostHTTPSPort`, `quicBindPort`, transport/log/plugins, etc.
- **Write procedure**: read file → parse (reject if invalid TOML) → apply changes → validate (ports in range, `allowPorts` ranges ordered, no duplicate singles, no overlap with `bindPort`/`webServer.port`) → write `frps.toml.yanto.bak` of the old content (0600) → write temp `<dir>/.frps.toml.yanto.tmp` with the original mode/owner → `fsync` → `rename` → `fsync` dir. Optional syntax validation by the frp binary (`frps verify -c <file>`) is **unverified**; Yanto skips it if the subcommand is unavailable.
- Applying requires a restart (frps reads config at start; hot-reload for frps is **unverified**). The UI shows "pending restart" until the restart succeeds; config is saved even if the restart is deferred. **D28**: applying (restarting `frps`) drops every live tunnel, so the UI requires an explicit confirmation dialog with that warning; over REST/MCP the restart is `daemon_control` (`admin`, `confirm: true`).

### 8.5 Restart and listing
- Restart: `systemctl restart <unit>` (also `start`, `stop`), then poll up to 10 s for `ActiveState=active` and a dashboard answer. On failure, show `systemctl status <unit> --no-pager -n 20` output and `journalctl -u <unit> -n 50`, and offer "restore previous config" from the `.bak` (restart again).
- Proxy listing from the dashboard/admin API at `http://<webServer.addr>:<webServer.port>` with HTTP Basic auth when user/password are set:
  - `GET /api/proxy/{type}` for `tcp`, `udp`, `http`, `https` (also `stcp`, `xtcp`, `sudp`: **unverified** list). Grounded: the route exists as `/api/proxy/{type}`; each entry carries `name`, `conf` (TCP includes the resolved remote port), `status` (`online`/`offline`), today's traffic in/out, current connections, last start/close time, and the owning user/client id. Exact JSON field names are **unverified**; confirm against the pinned frps version.
  - `GET /api/serverinfo` for version, uptime, client/proxy counts, totals (path used by legacy `frp.ts`; not returned by the Context7 lookup, so response shape is **unverified**). Legacy also calls `/api/clients` (connected frpc clients; shape unverified) — keep as an optional column.
  - Aggregate into one table (proxy name, type, remote port, status, traffic, connections, client). Poll when the page is visible (every 5 s via htmx), 2 s request timeout.
- Yanto stores no per-tunnel state (frpc generation dropped).

### 8.6 Failure modes
| Failure | UI |
|---|---|
| frps not installed | Panel with install instructions; other actions disabled |
| Dashboard not enabled/unreachable | Status "running (no dashboard)", with "enable dashboard" form that edits `webServer.*` and offers restart |
| Dashboard 401 | "Dashboard credentials in frps.toml do not match" (no password echo) |
| Invalid TOML in file | Read-only error with parse position; writes blocked |
| Restart fails | Output of `systemctl status`/journal, "restore previous config" |
| Port conflict at save | Field error naming the listener |

### 8.7 Security notes
- **Access control / network exposure**: dashboard bound to loopback by default; if the operator sets a non-loopback `webServer.addr`, the UI flags it as exposed. `allowPorts` is the main control against clients opening arbitrary Host ports — UI warns if absent (ASSUMPTION: empty `allowPorts` = unrestricted per frp docs semantics; **unverified** wording).
- **Secrets management**: `auth.token` and dashboard password live only in `frps.toml` (0600 recommended; Yanto warns if world-readable); redacted in logs, audit metadata, and diffs (diff view masks them).
- **Integrity**: backup + atomic write; restart is the only apply mechanism.
- Privileged actions (`systemctl`) run as root; unit name validated `^[A-Za-z0-9@._-]+$` before use.

### 8.8 Acceptance criteria
- [ ] Panel shows installed/active state, version, config path derived from the unit.
- [ ] Editing owned fields preserves comments and unrelated keys byte-for-byte (diff test).
- [ ] Invalid port/`allowPorts` rejected before write; `.bak` created on write; failed restart offers restore and restore works.
- [ ] Proxy list shows online/offline proxies of `tcp`, `udp`, `http`, `https` with traffic and connection counts.
- [ ] No secret value appears in UI diffs, logs, or Audit.
- [ ] Missing `frps` never causes an error outside the FRP page and never triggers an install.

---

## 9. Hostnames and Ingress Providers

### 9.1 Behavior (ADR 0008, D16)
A **Hostname** maps a public name to a declared loopback port of a Project (§1), or to the Hosted Registry (§4.5), through one **Ingress Provider**: Cloudflare Tunnel or Traefik. Both can be configured simultaneously; each Hostname belongs to exactly one provider. Cloudflare DNS management is independent of the provider.

`hostnames` (02 §4.6): `hostname`, `target_kind` (`project|registry`), `project_id`/`project_port_id` (null for registry), `ingress_provider` (`cloudflare_tunnel|traefik`), `tunnel_id`, `zone_id`/`manage_dns`/`dns_record_id`, `sync_status` (`pending|active|error|disabled`), `last_error`. Target URL is always `http://127.0.0.1:<host_port>`. Hostname validation: lowercase LDH labels `^([a-z0-9]([a-z0-9-]{0,61}[a-z0-9])?\.)+[a-z]{2,63}$`, ≤ 253 chars, unique across providers, no wildcards in v1 (ASSUMPTION), and (Cloudflare) must be inside a zone reachable by the selected client. Permission: `hostnames` on the Project (registry Hostnames: Owner only); provider/client/tunnel setup: Owner (`admin` scope).

**Reconcile model**: SQLite is the desired state; applying a provider means regenerating its *entire* config from the DB and pushing it idempotently (Cloudflare `PUT` of the full ingress list; Traefik full rewrite of `yanto.yml`). A reconcile runs on Hostname create/update/delete, when a Project's declared ports change, and via a manual "Re-sync". Drift (remote differs from DB) is shown, not silently accepted.

### 9.2 Cloudflare Tunnel (remote-managed)

**Entities** (02 §4.6): `cloudflare_clients` (a "client" = API token + account id; legacy term kept) and `cloudflare_tunnels` (`cf_tunnel_id`, `active`, health). Token scope required (ASSUMPTION; verify in Cloudflare docs): Account → Cloudflare Tunnel: Edit; Zone → DNS: Edit; Zone → Zone: Read.

**API calls** (base `https://api.cloudflare.com/client/v4`; paths and bodies as used by legacy `cloudflare.ts`):
| Purpose | Call |
|---|---|
| Validate client | `GET /accounts/{account_id}/cfd_tunnel?per_page=1`, `GET /zones?account.id={account_id}&per_page=50` |
| Create tunnel | `POST /accounts/{account_id}/cfd_tunnel` body `{ "name": "<name>", "config_src": "cloudflare" }` (remote-managed) |
| Get connector token | `GET /accounts/{account_id}/cfd_tunnel/{tunnel_id}/token` |
| Push ingress | `PUT /accounts/{account_id}/cfd_tunnel/{tunnel_id}/configurations` body `{ "config": { "ingress": [ { "hostname": "<h>", "service": "http://127.0.0.1:<port>" }, …, { "service": "http_status:404" } ] } }` |
| Tunnel health | `GET /accounts/{account_id}/cfd_tunnel/{tunnel_id}` (status field; allowed values **unverified**) |
| Delete tunnel | `DELETE /accounts/{account_id}/cfd_tunnel/{tunnel_id}` (behavior with live connections **unverified**; UI first tells the operator to stop the host `cloudflared`) |
| DNS | `GET/POST /zones/{zone_id}/dns_records`, `PUT/DELETE /zones/{zone_id}/dns_records/{id}` |
The final catch-all rule `http_status:404` is required by `cloudflared` ingress (last rule must be a catch-all; confirmed in cloudflared docs). Hostname rules use the `hostname` + `service` keys (confirmed). Legacy `noTLSVerify` and the `scheme` column are dropped (targets are plain HTTP on loopback).

**DNS for a Cloudflare Hostname**: `CNAME <hostname> → <cf_tunnel_id>.cfargotunnel.com`, `proxied: true` (legacy). Lookup existing record by `type=CNAME&name=<hostname>`; create or update; store `dns_record_id`. Records Yanto creates carry a comment `managed-by=yanto` where the API supports it (ASSUMPTION).

**Host `cloudflared` (a Host Daemon; never auto-run)**:
- Check: `cloudflared --version`; `systemctl is-active <unit>` (`config.toml` `[ingress.cloudflared] unit`, default `cloudflared`, ASSUMPTION that `service install` creates it); status mapping like §8.3; connector connectivity from the tunnel `GET` above.
- If the binary or unit is missing, or the tunnel has no connection, the UI shows instructions. The connector command is produced on demand: `cloudflared service install <TOKEN>` (root). Cloudflared docs confirm that, given a token, `service install` writes it to a token file and runs `tunnel run --token-file …`; on Linux it starts the service. Yanto **never executes this**, never stores the token (it fetches it from the API at click time), displays it once on a dashboard-only reveal action (Owner, audited `cloudflare_tunnel.token_reveal`), masks it in all logs, and the page sets `Cache-Control: no-store`.
- Multiple tunnels per Host are not supported in v1: one host `cloudflared` runs one tunnel token (ASSUMPTION; simplifies the check). The UI selects the active tunnel.

**Diagnostics** (read-only, per Hostname and per tunnel; each check returns pass/warn/fail + hint):
1. API token valid and scope sufficient (the validate calls above).
2. Tunnel exists; status from API; host `cloudflared` active.
3. Ingress drift: `GET …/configurations` compared with DB-derived ingress.
4. Target port declared and listening on `127.0.0.1` (TCP connect, 1 s).
5. Local HTTP probe: `GET http://127.0.0.1:<port>/` with `Host: <hostname>`, 3 s; reports status code only.
6. DNS record exists, type CNAME, content equals `<tunnel>.cfargotunnel.com`, proxied true.
7. Public probe `HEAD https://<hostname>/` (opt-in button, 5 s): reports status/TLS errors.
Each check is independent and never mutates.

**Failure modes**: invalid/insufficient token → client page error naming the HTTP status and Cloudflare error message (token never echoed); ingress `PUT` rejected → Hostname `error` with the API message, DB retained for retry; DNS conflict (existing non-CNAME record) → refused with the existing record shown, no overwrite without confirmation; tunnel API succeeds but connector offline → Hostname `active` with warning "connector not connected"; rate limit (429) → retry with backoff (3 tries) then error.

**Security notes**: **secrets management**: API token and tunnel token handling above; **least privilege**: scope guidance, one client per account; **exposure**: Hostname creation exposes a Project publicly — requires `hostnames`, audited, and the UI states that Cloudflare Access/WAF are the operator's responsibility; **integrity**: full-list `PUT` means a manual dashboard edit of the ingress will be overwritten (documented; drift check shows it first).

### 9.3 Traefik

**Host Daemon check** (read-only): `traefik version` (PATH lookup), `systemctl is-active <unit>` (`[ingress.traefik] unit`, default `traefik`), unit file path via `systemctl show`. Not installed → instructions only.

**Operator prerequisites (Yanto verifies where possible, never writes)** — static configuration (Traefik reads static config at start; shapes from Traefik docs):
```yaml
entryPoints:
  web:       { address: ":80" }
  websecure: { address: ":443" }
providers:
  file:
    directory: /etc/traefik/dynamic
    watch: true
certificatesResolvers:
  letsencrypt:
    acme:
      email: <operator email>
      storage: /etc/traefik/acme.json
      httpChallenge: { entryPoint: web }
```
- File provider with `directory` (loads all files in it) and `watch: true` (hot reload on change) — documented. ACME `httpChallenge.entryPoint` must be reachable by Let's Encrypt on port 80 — documented. Port 80/443 reachability and a public A/AAAA record are required.
- Yanto config (`config.toml` `[ingress.traefik]`, 01 §8.1): `dynamic_dir` (default `/etc/traefik/dynamic`), `cert_resolver` (default `letsencrypt`), `entrypoint` (default `websecure`), `static_config` (optional, default `/etc/traefik/traefik.yml`).
- **Prerequisite check** (best effort): parse the static config if present (YAML/TOML) and report a checklist: file provider directory equals `dynamic_dir`, `watch: true`, the named entry point and certificate resolver exist. If static config is passed by CLI flags/env only, status is "unverifiable" (not a failure). Also checks the directory exists and is writable.
- HTTP→HTTPS redirect is an operator choice at the `web` entry point (Traefik supports entry-point redirections; exact key set **unverified**, not generated by Yanto).

**Dynamic file** `<dynamic_dir>/yanto.yml` (only file Yanto writes; header `# Managed by Yanto. Manual edits are overwritten.`):
```yaml
http:
  routers:
    yanto-<hostname-slug>:
      rule: Host(`app.example.com`)
      entryPoints: [websecure]
      service: yanto-<hostname-slug>
      tls:
        certResolver: letsencrypt
  services:
    yanto-<hostname-slug>:
      loadBalancer:
        servers:
          - url: http://127.0.0.1:8080
```
Shape (routers with `rule`/`entryPoints`/`service`/`tls.certResolver`; services with `loadBalancer.servers[].url`) is documented in Traefik's file-provider and ACME docs. Router/service names are derived as `yanto-` + hostname with dots→`-` (collision-checked); hostnames are validated before they are put inside the backtick rule (injection guard); YAML is generated from a typed struct, not string concatenation.
- **Atomic write**: build string → parse it back and check invariants (every router references an existing service; URLs are `http://127.0.0.1:<declared port>`) → write temp file in the *parent* of the dynamic dir (`/etc/traefik/.yanto.yml.tmp`), so Traefik's directory watcher never sees a partial or stray file (ASSUMPTION: same filesystem) → `fsync` → `rename` to `yanto.yml` (mode 0644 root) → `fsync` dir. Previous content kept as `<dir>/../yanto.yml.bak`.
- **Hot reload**: none to trigger when `watch: true`. If the check finds `watch` false/unknown, Yanto shows "restart Traefik to apply" and offers `systemctl restart <unit>` (explicit button; Traefik reload-by-signal **unverified**).
- **Post-write verification** (best effort): wait up to 10 s, then probe `GET http://127.0.0.1:80/` with `Host: <hostname>` expecting a non-`404` router match (404 = router not loaded), and show `journalctl -u <unit> -n 30` when failing. Certificate issuance is asynchronous: status `active` + "certificate pending" until `HEAD https://<hostname>/` succeeds with a valid certificate (opt-in check).
- If the file is invalid, Traefik's behavior is to log an error and (ASSUMPTION) keep its previous config; Yanto surfaces the journal tail.
- **DNS** (optional, Cloudflare client selected): `A`/`AAAA <hostname> → <host_public_ip>` (`app_settings` `host.public_ip[_v6]`; Yanto does not call external IP-discovery services), `proxied: false` by default because HTTP-01 validation and Traefik-terminated TLS assume direct reachability (behavior behind Cloudflare proxy for HTTP-01 **unverified**). Without a client, the UI shows the record to create manually.

**Failure modes**: prerequisite missing → provider "not ready" banner with the failed checklist item, Hostname creation disabled for that provider; write permission denied → error with path; reconcile mismatch after reload (404) → Hostname `error` with journal tail; ACME failure (port 80 blocked/DNS wrong) → shown as "certificate pending/failed" with a pointer to `journalctl -u traefik` (Yanto cannot read Traefik's ACME state; `acme.json` is never read or written).

**Security notes**: **input validation** (hostname/rule injection, typed YAML); **secrets**: Yanto never reads `acme.json`; **exposure**: ports 80/443 are operator-owned; Hostname creation publicly exposes a loopback port; routers use `websecure` + TLS only (no plaintext router generated); **integrity**: atomic write, `.bak`, drift detection via stored SHA-256.

### 9.4 DNS records management (provider-independent)
Zones list per client, record list/create/update/delete (`GET/POST/PUT/DELETE /zones/{zone_id}/dns_records`, legacy-grounded; `per_page=200` listing). Records bound to a Hostname are marked "managed" and edit/delete there is routed through the Hostname (deleting a Hostname removes its DNS record only if Yanto created it). The Owner (`admin` scope) can edit any record; Members cannot. Changes audited with record name/type/content (no tokens).

### 9.5 Acceptance criteria
- [ ] Create a client; validation passes with a correct token and fails with a clear message for a wrong token without echoing it.
- [ ] Create tunnel (`config_src: "cloudflare"`); the page shows install instructions and the `cloudflared service install <TOKEN>` command only after an audited reveal; Yanto never runs it and never persists the token.
- [ ] Adding a Cloudflare Hostname for a declared port updates the full ingress (with the catch-all `http_status:404` last) and creates the proxied CNAME; removing it reverses both.
- [ ] Diagnostics identify each seeded fault: stopped host `cloudflared`, ingress drift, closed local port, wrong DNS target.
- [ ] A Hostname cannot target an undeclared or non-loopback port.
- [ ] Traefik provider check reports missing prerequisites correctly (no file provider, missing resolver, `watch` false) and blocks Hostname creation when not ready.
- [ ] Adding a Traefik Hostname rewrites `yanto.yml` atomically; Traefik serves the route within seconds without restart (with `watch: true`); no stray files appear in the dynamic dir during the write.
- [ ] Hostnames containing backticks, spaces, or uppercase are rejected/normalized before any file or API write.
- [ ] Both providers can coexist with distinct Hostnames; a hostname cannot be added twice.
- [ ] DNS A record for a Traefik Hostname is created only when a client and `host.public_ip` are configured.

---

## 10. Audit log

### 10.1 Behavior
Append-only record of who did what, through which interface, to which entity (port of legacy `audit_logs`, extended). Every mutating operation in every adapter (dashboard, REST, MCP, CLI, webhooks, cron-run) writes one entry from the service layer (ADR 0005), so coverage is independent of the interface.

### 10.2 Schema (`audit_logs`, 02 §4.5)
Columns and constraints are defined once in 02 §4.5: `actor_type` (`user|api_token|deploy_token|github|cli|system`), `actor_id`, `actor_label`, `channel` (`dashboard|rest|mcp|deploy-token|webhook|cli|cron|system`), `action` (dotted names listed in 03 §a.7, e.g. `deployment.trigger`, `registry.gc`, `authorized_key.add`, `container.exec`, `backup.download`), `entity_type`, `entity_id`, `project_id` (FK `SET NULL`, so entries survive Project deletion), `outcome` (`ok|denied|error`), `ip`, `metadata` (redacted JSON), `request_id`, `created_at`.
- **Append-only enforcement**: SQLite trigger `BEFORE UPDATE` aborts. Deletion is allowed only to the retention routine (`audit.retention_days`, default 365, 0 = keep), which runs in `yanto serve`.
- **Not in v1.0.0**: a hash chain, `yanto audit verify|prune`, and CSV/JSONL export (dropped for simplicity). Tail truncation by a root attacker is out of the threat model (ADR 0007); every entry is also emitted as a structured `tracing` line (target `audit`) so journald holds a second copy (ASSUMPTION).

### 10.3 What is and is not recorded
- Recorded: create/update/delete of Projects, Deploy Key/Token reveal/rotate, Deployments (start, cancel, finish) with trigger, webhook accepts and rejects (`deployment.trigger` with `outcome=denied`), container actions, exec session start/end (`container.exec`), prune, registry enable/users/delete/GC, Registry Credential create/rotate/delete (name and host only), cron entry changes with command text, authorized key add/remove (fingerprint, label), backups (create/delete/download/restore), FRP config diffs (secrets masked) and restarts, Cloudflare client/tunnel/Hostname/DNS changes, Traefik writes, login/logout/failed login/lockout, account and API Token lifecycle, settings changes, secret reveals, `denied` authorization failures.
- Never recorded: secret values (tokens, passwords, keys, `.env` contents), request bodies, terminal content, backup contents. Metadata passes through a typed redaction layer: fields are allow-listed per action, not free-form maps (the legacy free-form metadata is not carried over).
- **Fail-closed set**: for actions in the sensitive list (secret reveals, exec session start, authorized key changes, cron entry changes, token creation, backup download/restore, registry user changes, Hostname exposure) the operation is refused if the audit insert fails. All other actions log the failure via `tracing::error!` and continue (legacy "never throws" behavior, applied selectively).

### 10.4 Reading
- UI/REST/MCP: keyset-paginated list (`?limit≤500&before=<id>`), filters `project_id`, `actor`, `action` prefix, `outcome`, time range. Owner sees all; Members see only entries whose `project_id` is in their granted Projects (and never system-level entries). `GET /api/v1/audit` needs `read` scope plus the same filtering.
- Retention: `audit.retention_days` (default 365, 0 = keep) applied by the prune routine run daily.

### 10.5 Failure modes
| Failure | Surfaces as |
|---|---|
| Insert fails for a fail-closed action | Operation refused with "audit log unavailable" |
| Insert fails otherwise | Operation proceeds; `/api/v1/health` shows `audit: degraded`; journal error |
| Disk full | Same as insert failure; health warns at <10% free (see §7.4) |

### 10.6 Security notes
- **Logging & monitoring / non-repudiation**: authenticated actor on every entry; request id correlates with journal logs.
- **Data protection**: IP addresses are personal data under UU PDP; keep, but allow `audit.store_ip=false` (`app_settings`).
- **Integrity**: update trigger + journald mirror (a root attacker can drop triggers; the mirror makes tampering detectable, not impossible).

### 10.7 Acceptance criteria
- [ ] Every mutating action listed in 10.3 produces exactly one entry via each of UI, REST, and MCP, with the right `source` and actor.
- [ ] `UPDATE` on `audit_logs` fails at the SQLite level; retention pruning deletes only rows older than `audit.retention_days`.
- [ ] No secret/PII fixtures (tokens, passwords, key bodies, env values) appear in any entry (tested with canary values).
- [ ] A Member sees only entries for their Projects; Owner sees all.
- [ ] Deleting a Project keeps its audit entries.
- [ ] With the audit DB made read-only, a secret reveal is refused while a container list still works.

---

## Open questions and risks (consolidated)
2. `registry:2` tag semantics and in-container config path (§4.2); Cloudflare request-body limit (§4.5, unverified).
3. `cron-run` as root dropping privileges: more code on a root path (§5.3); alternative is a daemon socket.
4. Docker CLI plugin discovery under a temp `DOCKER_CONFIG` (§4.7); bollard exec resize API name per pinned version (§2.2).
5. Compose `config --format json` output stability for port derivation (§1.1).
6. frps `/api/serverinfo` response shape and `frps verify` existence (§8).
7. Lingering exec processes after terminal close (§2.2).
