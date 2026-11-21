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
- **GitHub**: `POST /webhooks/github?id=<project_id>`. Verify `X-Hub-Signature-256: sha256=<hex>` = HMAC-SHA256(secret = the Project's Deploy Token, body = raw bytes, before any form decoding), constant-time compare; missing/invalid → 401. Then: `X-GitHub-Event: ping` → 200; not `push` → 202 ignored; `deleted: true` → 202 ignored; ref not `refs/heads/<project branch>` → 202 ignored; `github_webhook_enabled=false` → 202 ignored; else enqueue with trigger `github` and `target_ref = after` sha is **not** used (deploy branch head, as legacy). Form-encoded delivery (`payload=` field) is unwrapped after verification. Dedupe on `X-GitHub-Delivery` (keep last 1000 / 24 h) → duplicate returns 200 `duplicate`. Body cap 5 MiB (ASSUMPTION) → 413.
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
