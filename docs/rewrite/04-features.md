> Classification: INTERNAL

# 04 — Feature specifications

Terms follow [`CONTEXT.md`](../../CONTEXT.md); decisions D1–D21 in [`00-scope.md`](00-scope.md) and the ADRs are binding and not re-decided here. Legacy behavior: [`docs/legacy/`](../legacy/).

**Conventions**
- **Fact** = grounded in the legacy code/docs or in documentation fetched via Context7 on 2026-11-21. **ASSUMPTION** = proposal not grounded in a source. **unverified** = a claim not confirmed in a source available while writing; check before implementing.
- Host side effects list files, commands and API calls exactly. "Docker" means the Docker Engine API through `bollard` unless a CLI is named. `docker compose` is always the CLI (the Compose plugin has no Engine API).
- Paths use `DATA=/var/lib/yanto` as an ASSUMPTION for the data dir (final value belongs to the install doc). `RUN=/run/yanto` is a root-only tmpfs dir (0700) for transient files.
- Every subprocess is spawned without a shell, in its own process group, with a timeout and a cancellation token (legacy `runCommand` discipline: SIGTERM, then SIGKILL after 5 s). Output is streamed to a log sink and capped when stored.
- Secrets (Deploy Tokens, Registry Credentials, Cloudflare API tokens, frps tokens, Deploy Key private keys) are stored encrypted at rest (envelope scheme defined in the security doc; ASSUMPTION) and never written to logs, audit metadata, or process arguments.
- Every mutating operation writes an Audit log entry (§10). Failures surface in the htmx UI as an inline error banner with the step name and last 12 output lines, and in REST/MCP as an RFC 7807-style JSON error (ASSUMPTION on format) with the same text.

---

## 1. Projects & Deployments

### 1.1 Behavior
A Project is Git-backed or image-backed and runs under Docker Compose with project name = `folder_name`. A Deployment brings it to a revision (Git ref, or image reference) and records status, logs, and the resolved revision.

**Source rules**
- Exactly one of `git_url` / `docker_image`. Free-form input is classified as in legacy: parses as an image reference → image Project; starts with `docker ` → rejected with hint; else Git URL.
- Git URL schemes allowed: `ssh://`, scp-style `user@host:path`, `https://` (public repos only; no credentials in URLs — ASSUMPTION, avoids token leakage into `.git/config`). Reject anything else (`file://`, `ext::`, leading `-`).
- `branch` default `master` (ASSUMPTION: also accept configured default `main`; no auto-detection). Refs must match `^[A-Za-z0-9._/@-]+$`, not start with `-`, not contain `..`.
- `compose_file` default `docker-compose.yml` (`^[A-Za-z0-9._/-]+\.ya?ml$`, relative, no `..`); `env_file` default `.env` (relative, basename starts with `.env`); `folder_name` slug `^[a-z0-9._-]{1,64}$` unique; `compose_content` optional pasted YAML (overrides repo file at deploy); `auto_start` (restart policy override); `manual_deploy_enabled`; `github_webhook_enabled`.
- Image Projects without `compose_content` get a generated compose `services: { app: { image: <ref> } }`.

**Declared ports (Change vs legacy)**
- A Project's published ports are *declared* state: `project_ports(project_id, service, container_port, host_port, protocol, name)`. They are derived on every save/deploy from `docker compose -p <name> -f <file> config --format json` (ASSUMPTION: flag and JSON shape per the installed Compose v2; fall back to YAML parsing of `ports` short/long syntax with range expansion capped at 512 as in legacy; `${VAR}` unresolved ports are reported as warnings, not silently skipped).
- Only host IPs `127.0.0.1` / `::1` are accepted. A mapping with no host IP or a non-loopback host IP is rejected with "publish on 127.0.0.1 so an Ingress Provider carries the traffic". Rationale: public publish would bypass Hostname routing and expose the port directly (access control).
- Hostnames (§9) may target only declared ports.

**Collision check** (runs at create/update, pre-deploy, and when a Hostname targets a port). A declared `host_port/protocol` conflicts if it is: (a) declared twice inside the Project; (b) declared by another Project; (c) published by a running container not belonging to this Project (`list_containers` ports, ignoring containers whose `com.docker.compose.project` equals this Project's `folder_name`); (d) bound by any host listener (parse `/proc/net/tcp`, `/proc/net/tcp6` rows in state `0A`; map socket inode → PID → `comm` via `/proc/*/fd`, best effort; no `lsof` dependency in a static binary). Error lists up to 8 conflicts with owner (Project/container/process).

**Queue and concurrency**
- Statuses: `queued | running | success | failed | cancelled`. At most one `running` Deployment per Project (enforced by a partial unique index `WHERE status='running'` plus an in-process per-Project mutex; SQLite replaces the legacy advisory lock).
- A trigger arriving while one is running enqueues one Deployment; a further trigger while one is queued returns the queued one (`reused: true`, same as legacy reuse semantics). Queue is FIFO across Projects with a global concurrency cap `deploy.max_parallel` (default 2, ASSUMPTION).
- Startup recovery: `running` rows become `failed` with notice "Yanto restarted during this Deployment"; `queued` rows resume.
- Shutdown: stop accepting, wait up to 60 s for running Deployments, then cancel; the systemd unit sets `TimeoutStopSec=90` (ASSUMPTION).

**Cancellation**: `queued` → `cancelled` immediately. `running` → trip the cancellation token; current child group gets SIGTERM, SIGKILL after 5 s; status `cancelled`, exit code 130 (ASSUMPTION). No automatic cleanup: a cancelled `compose up` may leave a partial stack; the log says so and offers Rollback/Redeploy.

**Rollback**
- Git Project: target = explicit ref, else a chosen prior Deployment's `commit_sha`, else the second distinct successful commit (legacy rule). Preview (read-only, needs existing checkout): `git fetch --all --tags`, current vs target summary, `git rev-list --count` both directions, `git diff --numstat` capped at 40 files.
- Image Project (New): each successful Deployment records `image_digest` (`RepoDigests[0]` from `inspect_image`). Rollback deploys `repo@sha256:<digest>` via a generated override; preview does a manifest existence check (for Hosted Registry images the digest may have been garbage-collected, §4) and fails early if missing.
- Rollback creates a new Deployment with trigger `rollback` and `rollback_from_deployment_id`. It restores code/image only; compose/env edits are not versioned (each Deployment stores sha256 of compose and env content, not the content, to explain drift — ASSUMPTION).

### 1.2 Inputs / state (SQLite)
- `projects`: fields above + `deploy_token_enc`, `deploy_key_id?`, `created_at`.
- `deploy_keys(id, project_id, public_key, private_key_enc, created_at)`.
- `project_registry_credentials(project_id, credential_id)` (links to §4 Registry Credentials).
- `deployments(id, project_id, status, trigger [manual|webhook|github|rollback], target_ref, rollback_from, commit_sha, commit_message, image_digest, exit_code, queued_at, started_at, finished_at, compose_sha, env_sha)`.
- `deployment_log_chunks(deployment_id, seq, ts, text)`; total stored text capped at `deploy.log_max_bytes` (default 2 MiB, ASSUMPTION): oldest chunks dropped, a truncation notice stored once.
- Files on Host: `PROJECTS_ROOT/<folder_name>/` (default `DATA/projects`; mode 0750), `<compose_file>`, `<env_file>` (0600), `.yanto.restart.override.yml`.

### 1.3 Host/Docker side effects (exact order of a Deployment)
1. Insert `deployments` row (`queued`), then `running` when dequeued. Emit SSE `status`.
2. **Pre-flight**: `docker compose version` (cached 60 s); ensure `PROJECTS_ROOT` exists.
3. **Git source** (if `git_url`):
   - Env for every git command: `GIT_TERMINAL_PROMPT=0`, `GIT_ALLOW_PROTOCOL=ssh:https`, and, if the Project has a Deploy Key, `GIT_SSH_COMMAND="ssh -i DATA/keys/<key_id> -o IdentitiesOnly=yes -o BatchMode=yes -o StrictHostKeyChecking=accept-new -o UserKnownHostsFile=DATA/ssh/known_hosts"`. The private key is materialized to `DATA/keys/<key_id>` (0600, parent 0700) only for the duration of the Deployment and removed after (decrypted from the DB; ASSUMPTION), never placed in argv. `StrictHostKeyChecking` is configurable (`accept-new` default, `yes` for pre-seeded known_hosts).
   - Missing dir: `git clone --branch <branch> -- <git_url> <path>`. Dir exists but no `.git`: `git init` + `git remote add origin <git_url>`.
   - Explicit target ref: `git fetch --all --tags`, `git checkout <ref>`. Otherwise `git fetch origin <branch>`, `git checkout <branch>` (or `checkout -B <branch> origin/<branch>`), `git pull --ff-only`.
   - `git rev-parse HEAD`, `git log -1 --pretty=%s` → stored on the row.
4. **Config files**: write `compose_content` (if set) to `<path>/<compose_file>`; for image Projects without content write generated compose. Write pending env (if the trigger carried one) to `<env_file>` mode 0600 via temp file + rename. Assert compose file exists (else error naming the probed alternatives `compose.yml`, `compose.yaml`, `docker-compose.yaml`).
5. **Registry auth** (if the Project is linked to Registry Credentials or its image is on the Hosted Registry): create `RUN/docker-<deployment_id>/` (0700) containing `config.json` with `auths` entries only, symlink `cli-plugins` to the system/real plugin dir if present; export `DOCKER_CONFIG=<that dir>` for steps 6 and 8; remove the dir in a drop-guard (also swept on startup). Details and justification: §4.7.
6. **Image Projects**: `docker pull <image>` (or `docker compose pull` for Git Projects whose compose names private images).
7. **Ports**: derive declared ports, run the collision check, persist `project_ports`.
8. **Compose**: args `compose -p <folder_name> -f <compose_file> [-f .yanto.restart.override.yml] [--env-file <env_file> if non-default]`. Auto-start on → write the override (`restart: unless-stopped` for every service, names from `compose config --services`), off → delete it. `... ps -q --status running`: if any running → `build` then `up -d --remove-orphans`; none → `up -d --build --remove-orphans`.
9. Record `image_digest` (image Projects), set `success`/`failed`, emit SSE `done`, write Audit entry, invalidate the container cache (§2), trigger Hostname reconcile if declared ports changed (§9).

Per-step timeouts (ASSUMPTION): git 10 min, pull 30 min, build/up 60 min. Each command line is echoed to the log as `$ <cmd> <args>` (env values never echoed).

**Project lifecycle side effects**
- Create: dir + files as above, generate Deploy Token (32 random bytes, base64url, prefix `ydp_`) and optionally a Deploy Key (`ssh-keygen -t ed25519 -N "" -C yanto-<project_id> -f <tmp>`; ASSUMPTION: shell out as legacy does, since `ssh-keygen` is a prerequisite anyway for `ssh -i`).
- Stop/restart (no redeploy): `docker compose -p <name> -f ... stop|restart`.
- Delete: refuse while `running`; `docker compose -p <name> down --remove-orphans` is **not** automatic — UI offers "also stop containers" (default on) and "also delete volumes" (default off, typed-name confirmation); then delete Hostnames (§9), rows, and `rm -rf` the resolved project dir (path-containment asserted under `PROJECTS_ROOT`).

### 1.4 Webhooks
- **Manual/CI**: `POST /deploy?id=<project_id>`, `Authorization: Bearer <deploy_token>`, optional JSON `{"ref": "<git ref>"}` (validated; ASSUMPTION). Requires `manual_deploy_enabled`. Constant-time token compare against the decrypted token. Response `202 {deployment_id, status, reused}`. Not CSRF-protected (bearer-only; no cookie auth accepted on this route). Rate limit 60/min per (client IP, project) (legacy value).
- **GitHub**: `POST /webhooks/github?id=<project_id>`. Verify `X-Hub-Signature-256: sha256=<hex>` = HMAC-SHA256(secret = the Project's Deploy Token, body = raw bytes, before any form decoding), constant-time compare; missing/invalid → 401. Then: `X-GitHub-Event: ping` → 200; not `push` → 202 ignored; `deleted: true` → 202 ignored; ref not `refs/heads/<project branch>` → 202 ignored; `github_webhook_enabled=false` → 202 ignored; else enqueue with trigger `github`; the Deployment uses the branch head, not the pushed `after` sha (legacy behavior). Form-encoded delivery (`payload=` field) is unwrapped after verification. Dedupe on `X-GitHub-Delivery` (keep last 1000 / 24 h) → duplicate returns 200 `duplicate`. Body cap 5 MiB (ASSUMPTION) → 413.
- Image Projects: the same endpoints pull the current tag and redeploy (typical CI flow: push to Hosted Registry, then call `/deploy`).
- Deploy Token reveal requires `secrets`; rotate invalidates the old token and the GitHub secret simultaneously (UI says so).

### 1.5 Logs and SSE
- Writer batches log text every 250 ms or 8 KiB into `deployment_log_chunks` and publishes on an in-process `tokio::broadcast` per Deployment.
- `GET /projects/{id}/deployments/{dep}/events` (`text/event-stream`): `event: log` (`id: <seq>`), `event: status`, `event: done`. `Last-Event-ID` replays from the DB; a lagged receiver resyncs from the DB. Keep-alive comment every 15 s. Stream closes after `done`.
- Log text is HTML-escaped on render (logs are untrusted: build output, git messages); ANSI colors are converted to a fixed class whitelist.
- Redaction: known secret values (Deploy Token, registry passwords, env values from the Project's env file) are replaced by `***` in log text before storage (best effort, exact-match).

### 1.6 Failure modes and UI surfacing
| Failure | Surfaces as |
|---|---|
| Collision | Pre-deploy fail at step 7 and inline on the Ports form with conflicting owner |
| Git auth/host key failure | Failed Deployment; message hints "add the Deploy Key's public key to the repo / check host key policy" |
| Non-fast-forward pull | Failed; offers audited "Reset checkout to origin/<branch>" (`git reset --hard`) |
| Registry auth/pull failure | Failed at step 6 with registry host named; credential values never shown |
| Compose plugin missing | Fails pre-flight; `/health` shows `docker compose: missing` |
| Cancel | Status `cancelled` + partial-state notice |
| Daemon restart | `failed` with restart notice (recovery) |
| Webhook rejected | 401/202/413 JSON; GitHub shows the response in its delivery log; Audit entry `webhook.rejected` (no body stored) |

### 1.7 Security notes
- **Secrets management**: Deploy Token doubles as GitHub HMAC secret (legacy design) — one secret, two uses; stored encrypted, shown only to `secrets`. Deploy Key private key never in argv/log; temp materialization 0600; Deploy Keys are per Project so a leak is scoped.
- **Input validation**: all paths resolved and containment-checked; refs/URLs validated before use; `--` before positional args; Git protocol allow-list defeats `ext::` command execution.
- **Supply chain / trust**: Deployments run arbitrary compose from the repo as root-equivalent on the Host (ADR 0007). Anyone with `deploy` on a Project can execute code on the Host; state this in the UI and README.
- **Access control**: webhook routes accept bearer/HMAC only; Member permissions `deploy`/`config`/`secrets` gate the respective actions.
- **Availability**: rate limit, body caps, per-step timeouts, log caps.

### 1.8 Acceptance criteria
- [ ] Create Git Project with Deploy Key; public key shown; first Deployment clones via `GIT_SSH_COMMAND` and the private key file is gone afterwards.
- [ ] Create image Project; Deployment pulls and runs generated compose with `-p <folder_name>`.
- [ ] Compose publishing `0.0.0.0:8080` or bare `8080:80` is rejected; `127.0.0.1:8080:80` accepted and recorded.
- [ ] Collision with another Project, a foreign container, and a host listener are each detected; a redeploy of the same Project does not collide with itself.
- [ ] Two simultaneous triggers: one runs, one queues, a third returns the queued one; never two `running` for a Project.
- [ ] Cancel of a running Deployment terminates the whole child process group within ~5 s and sets `cancelled`.
- [ ] Rollback preview shows commit counts and ≤40 file diffstat; rollback deploys prior commit (Git) / prior digest (image).
- [ ] `/deploy` rejects wrong/missing token; GitHub route rejects bad signature, accepts valid push on matching branch, ignores other branches/events, dedupes delivery IDs.
- [ ] SSE stream shows live log, resumes after reconnect without duplicates/gaps, closes on `done`.
- [ ] Yanto restart mid-Deployment yields `failed` with notice; queued ones resume.
- [ ] No secret value appears in logs, audit metadata, `ps` output, or error banners.

---

## 2. Containers

### 2.1 Behavior
- **List** all containers (`list_containers(all=true)`), joined with one-shot stats for running ones, and mapped to Projects by label `com.docker.compose.project == project.folder_name`. Columns: id, name, image, state/status, published ports, created, CPU %, memory usage/%, compose project/service, owning Project. Sorted newest first. Cache 5 s with a revision counter bumped by any mutation; concurrent readers share one in-flight fetch. CPU % is computed with the standard delta formula from two stats samples (ASSUMPTION on exact bollard stats fields; unverified).
- **Actions**: start, stop, restart (stop timeout 10 s, ASSUMPTION). Authorization: `runtime` on the owning Project; containers belonging to no Project need Owner. **Protected**: the Hosted Registry container (`yanto-registry`) accepts actions only from the Registry page (§4), not here (identified by label `yanto.managed=registry`).
- **Logs**: last 500 lines (`logs(stdout,stderr,tail=500,timestamps)`); live follow via SSE (`follow=true`, `since=<last ts>`), demultiplexed stdout/stderr; TTY containers return raw stream. HTML-escaped on render.
- **Exec terminal** over WebSocket (D13): `GET /containers/{id}/terminal` upgrades to WS.

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
| Docker socket unreachable | Page-level banner "Docker unavailable" (also `/health`), actions disabled |
| Container gone between list and action | Toast "container no longer exists", list refresh |
| Exec on stopped container | Terminal refuses before WS upgrade with 409 and message |
| WS Origin mismatch / no permission | 403, no exec created |
| Shell missing (distroless) | Terminal prints "no shell found in image" and closes (exec exits 127) |
| Stats timeout | Row renders without CPU/mem (never blocks the list) |

### 2.5 Security notes
- **Access control / privilege**: an exec shell is root in the container and, with socket-adjacent mounts, potentially on the Host. Gated by `runtime`, audited at start and end (container, account, exec id, duration, bytes in/out); keystrokes and output are **not** recorded (open question: optional session recording).
- **Session security**: WS requires Origin check + session; tokens (`API Token`) may not open terminals in v1 (ASSUMPTION: interactive only).
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
- **App logs**: Owner-only viewer: `journalctl -u yanto -n 500 --no-pager -o short-iso` with SSE follow (`-f`). Falls back to a message if `journalctl` is absent. Secret redaction is applied at log emission time (tracing layer), not on read.

### 3.2 Host side effects
Reads: `/proc/*`, `statvfs`, `journalctl` subprocess. Writes: none for usage/health. Prune: Engine API `prune_*` calls only. Cache usage snapshot 5 s.

### 3.3 Failure modes
| Failure | UI |
|---|---|
| Docker down | Usage still renders host stats; Docker panels show error; health `degraded` |
| `journalctl` missing/permission | App-logs panel shows reason |
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
- `DATA/registry/htpasswd` (0600): one line per user `name:<bcrypt hash>`.
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
- Named Docker volume `yanto-registry-data` mounted at `/var/lib/registry` (blobs; survives container recreation).

Container (bollard `create_container`/`start`): name `yanto-registry`, image `registry:2`, labels `yanto.managed=registry`, `HostConfig.PortBindings 5000/tcp → 127.0.0.1:5000`, `RestartPolicy unless-stopped`, binds `DATA/registry/config.yml:/etc/docker/registry/config.yml:ro` (**unverified** path; newer Distribution images may read `/etc/distribution/config.yml` — verify against the pulled image's `Cmd`/docs before implementing, or pass the path explicitly as the container command), `DATA/registry/htpasswd:/auth/htpasswd:ro`, volume above. Disable removes the container only; volume and files are kept unless "delete data" is chosen with typed confirmation.

htpasswd facts (Distribution docs): the only supported format is **bcrypt**; entries with other hash types are **ignored**; the file is **loaded once at startup**; with TLS absent basic auth sends passwords in cleartext headers; a missing file is auto-created with a generated user (so Yanto must always write the file *before* first start). Therefore:
- Hash with bcrypt (cost 10, ASSUMPTION), `$2y$` prefix as `htpasswd -B` produces (ASSUMPTION that Go bcrypt accepts `$2y$`; Apache-compatible files rely on it — test in CI).
- Any user add/remove/password change = rewrite the file atomically (temp file in `DATA/registry/`, fsync, rename) **then restart the container** so it reloads (no hot reload documented).
- Passwords are shown once at creation, stored only as hashes. Min length 16 for generated, 12 for chosen (ASSUMPTION). Username `^[a-z0-9._-]{1,32}$`.
- Yanto keeps one internal user `yanto-internal` whose password is stored encrypted (it must be recoverable to browse via the API and to pull for Projects).

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
Docker clients require HTTPS for non-loopback registries, so a Hostname is mandatory. Two supported paths, both created as ordinary Hostnames (§9) pointing at `http://127.0.0.1:5000`; the registry is not a Project, so Hostname targets also allow "Hosted Registry" (special target, port 5000):
- **Cloudflare Tunnel**: easiest (no open ports, edge TLS). **Caveat**: Cloudflare limits request body size on proxied traffic, and image layer uploads (`PATCH`/`PUT` of blobs) can exceed it. The limit depends on plan and is **unverified** here; check Cloudflare's current limits docs before documenting a number. Layers above the limit fail to push with a connection/413-style error. Mitigations: keep layers small, push from the Host itself (loopback), or use Traefik/FRP.
- **Traefik** (ports 80/443, Let's Encrypt): no body limit imposed by a CDN; DNS record should be DNS-only (not proxied) (§9). Recommended for large images.
- **FRP** (frps only): an FRP Tunnel can expose a port but terminates no TLS; pushing needs TLS to the registry, so use FRP only when TLS is handled elsewhere (ASSUMPTION; mostly not recommended). Yanto does not wire it; docs describe it.
- Basic auth always on; the UI warns when a remote Hostname is not HTTPS-terminated.

### 4.6 Registry users, UI, and state
- `registry_settings(enabled, image_ref, image_digest, port=5000, gc_in_progress)`; `registry_users(name, hash_updated_at)` (hash lives only in the htpasswd file; ASSUMPTION: the file is source of truth for hashes, the table for metadata); `yanto-internal` credential encrypted in settings.
- Permissions: Owner/`admin` only for enable/disable, users, delete, GC. Members with project `config` may read the repo list read-only (ASSUMPTION).

### 4.7 Registry Credentials (external registries)
- Entity: `registry_credentials(id, name, registry_host, username, secret_enc)`; `registry_host` normalized (`ghcr.io`, `docker.io`→ key `https://index.docker.io/v1/`, `host:port`). Linked to Projects (`project_registry_credentials`). Validate on save with a login probe: `GET https://<host>/v2/` and follow the `WWW-Authenticate` challenge (ASSUMPTION; failures reported but not blocking).
- **Auth is passed per command via a temp `DOCKER_CONFIG` directory**, not by writing `/root/.docker/config.json` and not via `docker login`:
  - Per Deployment (and per prune/pull action that needs auth) create `RUN/docker-<id>/` (mode 0700, on tmpfs so it never hits disk), write `config.json` containing only `{"auths": {"<host>": {"auth": "<base64(user:secret)>"}}}` (mode 0600), run `docker pull` / `docker compose pull|up|build` with `DOCKER_CONFIG` pointing there, then delete it in a drop-guard; startup sweeps stale `RUN/docker-*`.
  - Justification: (1) **secrets management**: the secret exists on tmpfs only for the command's lifetime instead of persisting in plaintext base64 in root's home, which any later root process or backup would pick up; (2) **isolation**: Project A's pull cannot use Project B's credential, and manual `docker` use on the Host is unaffected; (3) **no global mutation**: concurrent Deployments cannot clobber each other's login state, which `docker login`/`logout` on a shared file would; (4) **auditability**: credential use is tied to a Deployment id.
  - Trade-offs/risks: `config.json` still holds a base64 (reversible) secret during the command; mitigated by 0700 dir on tmpfs. Changing `DOCKER_CONFIG` also changes where the CLI looks for **plugins** (`cli-plugins`) and `buildx` state — if `docker compose` lives under root's `~/.docker/cli-plugins`, it would disappear (**unverified**; mitigated by symlinking `RUN/docker-<id>/cli-plugins` to the real plugin dir when it exists, and by the pre-flight `docker compose version` run under the same env). Credential helpers (`credsStore`) are intentionally not used.
  - `docker login` is never run; no secret is passed on argv (`--password` would leak via `ps`).
- Selection: for each image/compose service the registry host is derived from the reference (first component containing `.` or `:`, or `localhost`; otherwise Docker Hub). Only credentials linked to the Project **and** matching hosts are written.
- Reveal: secret never revealed after creation (rotate only); `secrets` permission needed to create/rotate.

### 4.8 Failure modes
| Failure | Surfaces as |
|---|---|
| Port 5000 already in use at enable | Enable fails with owner of the listener (same host-listener scan as §1) |
| htpasswd invalid (e.g. non-bcrypt line) | Registry exits at start; Yanto reads container exit + last log lines and shows them; file rewritten from `registry_users` metadata cannot recover hashes, so the UI offers "reset all users" |
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
- [ ] Enabling creates config, htpasswd (with `yanto-internal`), volume, and a container bound only to `127.0.0.1:5000`; `curl` to `/v2/` without auth returns 401.
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
Entry: `{id, name, schedule, user, command, enabled, record (default true), overlap (allow|skip), timeout_s?, comment}`.
- **Validation of schedule**: Yanto parses with its own validator (ranges: min 0–59, hour 0–23, dom 1–31, mon 1–12 or `jan–dec`, dow 0–7 or `sun–sat`; lists `,`, ranges `-`, steps `/`, `*`); rejects empty fields, steps of 0, descending ranges, and wrong field count; shows "next 5 run times" (computed in the Host's timezone; note cron's dom/dow OR semantics when both are restricted). `user` must resolve via `getpwnam`; the UI shows a dropdown of login-capable users plus `root`. `command` must be non-empty, single-line (no `\n`/`\r`), no NUL, ≤ 4096 bytes.
- **Generated file** (header comment, "managed by Yanto, edits are overwritten"):
  - Recorded entry (default): `<schedule> root /usr/local/bin/yanto cron-run <id>` (binary path = `current_exe`).
  - Unrecorded entry (`record=false`): `<schedule> <user> <command with % escaped as \%>`.
  - Disabled entries are emitted as `# disabled: <id>` comments, or omitted (comment form; ASSUMPTION so the file documents state).
- `yanto cron-run <id>` is invoked by cron as **root** (that is why the user field is `root`), loads the entry from SQLite (opens the DB directly; WAL + busy timeout), then drops privileges for the child: `initgroups` + `setgid` + `setuid` to `entry.user`, `HOME`, `LOGNAME`, `USER`, `SHELL`, `PATH` set as cron would, then runs `/bin/sh -c <command>`. Because the command is read from the DB, the cron file never contains user-controlled text, which removes `%`-escaping and injection issues for recorded entries.
- **Run history decision (resolves the ASSUMPTION)**: cron discards output unless wrapped (mail disabled), so Yanto records runs through `cron-run`: `cron_runs(entry_id, started_at, finished_at, exit_code, timed_out, output_tail)`; output captured combined, tail 64 KiB, UTF-8 lossy. Retention: last 50 runs and 30 days per entry (ASSUMPTION). `overlap=skip` uses `flock` on `RUN/cron-<id>.lock` and records a `skipped` run. `timeout_s` → SIGTERM then SIGKILL after 5 s. If Yanto's DB is unreadable, `cron-run` still executes the command? **No** — it exits 75 and logs to syslog (journal) since it cannot know the command (fail closed).
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
| Entry's user deleted from Host | Run history shows `unknown user`; entry flagged invalid |
| `cron-run` failure to open DB | Exit 75, journal line, run not recorded |

### 5.6 Security notes
- **Privilege / access control**: entries run commands as any chosen user including root, from a root-run service: creating/editing needs a dedicated permission (`admin` scope / Owner; Members cannot, ASSUMPTION). Audit logs the full command text (no secrets expected; UI warns not to put secrets in commands).
- **Input validation**: single-line commands, NUL/newline rejection; recorded form eliminates cron-file injection; unrecorded form escapes `%`.
- **Integrity**: atomic replace, perms checked each write, symlink refusal.
- **Privilege drop correctness**: `initgroups`, `setgid` before `setuid`; refuse if the target UID resolves to 0 while the entry user is not `root`.

### 5.7 Acceptance criteria
- [ ] Creating an entry produces `/etc/cron.d/yanto` with mode 0644 root, trailing newline, valid environment header; cron runs it within a minute.
- [ ] Invalid schedules (`61 * * * *`, `*/0 * * * *`, 4 fields) are rejected with field-level errors; the next-5-runs preview matches `cron` semantics on tested cases.
- [ ] Temp file during write has a dotted name and is never executed by cron.
- [ ] Recorded run stores exit code, duration, and output tail; `overlap=skip` prevents concurrent runs; timeouts kill the process group.
- [ ] A command containing `%` runs correctly for both recorded and unrecorded entries.
- [ ] Editing the file by hand triggers the drift banner; "re-apply" restores it.
- [ ] Other cron files are visible but have no edit controls and are never written.
