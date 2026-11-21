> Classification: CONFIDENTIAL

# 09 — Final review of the Rust codebase (M0–M9)

**Scope.** The whole workspace on `rewrite/rust` at `d57148a`, reviewed on branch `review-opus`:
- `yanto-core`: services, `authorize`, repos and migrations
- `yanto-host`: side effects
- `yanto-server`: dashboard, `/api/v1` and MCP
- `yanto-cli`

**Priorities, in order:** security, then correctness, then quality.

**Rules followed.**
- Each fix is a focused commit with a test that would have caught the bug.
- Decisions D1–D31 were not changed.
- Where a finding is a design concern, it is recorded below and was not changed unilaterally.

Line numbers point to the code after the fixes.

**Severity scale.**
- **High**: exploitable privilege escalation, a write or read as root outside the intended scope, or a core feature that is broken in production.
- **Medium**: an access-control or spec gap that needs an existing foothold, or a correctness bug with a user-visible effect.
- **Low**: defence in depth, DoS-only issues, or hygiene.

## Summary

| | High | Medium | Low | Info | Total |
|---|---|---|---|---|---|
| Fixed | 5 | 8 | 9 | 0 | 22 |
| Recorded, not fixed | 1 | 1 | 9 | 3 | 14 |

## Findings (fixed)

| Id | Sev | Risk area | Location (file:line) | What | Status | Commit |
|---|---|---|---|---|---|---|
| R-01 | High | Path traversal | `crates/yanto-core/src/deploy/files.rs:99` | `resolve_in` only checked paths lexically. The Project dir *is* the Git checkout (attacker-controlled). A committed `docker-compose.yml -> /etc/shadow`, `.env -> keys/app.key`, or a symlinked parent dir let `compose_read` (`config`, even a `read` token) and `env_content_read`/`env_write` (`secrets`) read or write any file as root. Fix: the existing prefix is now canonicalized and must stay inside the Project dir, and dangling symlinks are refused. | fixed | `3d169ea` |
| R-02 | High | Access control | `crates/yanto-core/src/service/hostnames.rs:1067` | `dns_ensure` updated any same-type record at the name. A Member with `hostnames` on one Project could create `www.<owner zone>` and repoint the Owner's CNAME/A record to their tunnel or to the Host, then delete it by `dns_record_id`. Fix: replacing a record without the managed marker now needs the Owner, and re-sync never adopts foreign records. | fixed | `d73fd2a` |
| R-03 | High | Host-file writers (TOCTOU, symlinks) | `crates/yanto-host/src/authkeys.rs:106`, `crates/yanto-host/src/fsutil.rs:115` | Root edited the target user's own `~/.ssh` by path. Staging used `std::fs::write` (follows symlinks), chmod/chown went by path, and `~/.ssh` was re-resolved on every step. A racing symlink could make root write the user-controlled file (e.g. cron lines) into `/etc/cron.d`, chown a root file to the user, or copy `/etc/shadow` into `authorized_keys`. Fix: a new `DirHandle` (`O_DIRECTORY\|O_NOFOLLOW`, then `openat`/`renameat`/`unlinkat` with `O_NOFOLLOW\|O_EXCL`, `fchown`/`fchmod`). `atomic_write` now uses `fchown`. | fixed | `434c19d` |
| R-04 | Medium | Host-file writers | `crates/yanto-cli/src/ops/uninstall.rs:153` | `yanto uninstall` rewrote `authorized_keys` as root with the generic writer: it followed symlinks and left the file root-owned, so the user could no longer edit their own keys. It now goes through `DirHandle` and keeps the owner. | fixed | `434c19d` |
| R-05 | High | Correctness (concurrency) | `crates/yanto-core/src/service/deployments.rs:737` | `cancel` read `queued` and then called `finish`, which matches `queued` **or** `running`. If the worker claimed the row in between, the row was marked `cancelled` while git/compose kept running. That freed the one-running-per-Project slot (a second Deployment could start in the same dir) and dropped the real outcome. Fix: new `cancel_queued` matches `status = 'queued'` only. | fixed | `e5f29f2` |
| R-06 | Medium | Correctness | `crates/yanto-core/src/service/deployments.rs:321` | While a Deployment was queued, a deploy with `target_ref` or an env payload, or a rollback to a git ref, was answered `202 reused` and its payload was silently discarded. Fix: only plain triggers coalesce; the others get 409. | fixed | `382f3d3` |
| R-07 | Medium | Correctness (shutdown) | `crates/yanto-cli/src/cmd/serve.rs:82`, `crates/yanto-core/src/deploy/worker.rs:25` | The worker was stopped only after axum's graceful shutdown, which waits for every connection. Any open SSE tab therefore held SIGTERM until systemd's SIGKILL: the worker kept claiming jobs and the 30 s drain never ran. Fix: the signal now stops the worker immediately and HTTP gets 5 s. The worker's post-cancel grace went from 6 s to 12 s so cancelled commands can finalize. | fixed | `2e6e953` |
| R-08 | Medium | Rate limiting / input validation | `crates/yanto-server/src/net.rs:109` | `CF-Connecting-IP` from a trusted peer beat `X-Forwarded-For`. Behind a non-Cloudflare local proxy, a client could rotate the header to bypass the login/setup/bearer/deploy per-IP limits and forge audit IPs. Fix: the right-most untrusted XFF entry is used first. | fixed | `ffcd4c2` |
| R-09 | Low | Input validation | `crates/yanto-core/src/service/auth.rs:291` | Login sliced the password at byte 1024 for the dummy hash. An unauthenticated request with a longer multibyte password panicked the handler. | fixed | `020d4b1` |
| R-10 | Low | Authentication (user enumeration) | `crates/yanto-core/src/service/auth.rs:291` | Active users with no password yet (pending invite) skipped Argon2, so timing told them apart. They now burn the dummy hash too. | fixed | `020d4b1` |
| R-11 | Medium | Access control / sessions | `crates/yanto-server/src/dashboard/exec.rs:127` | The exec WebSocket never re-checked access (03 a.3 requires every 15 s). After logout, revocation, a disabled account or a removed `runtime` grant, the shell stayed open for up to 4 h. Fix: re-authenticates every 15 s plus `ExecTicket::still_allowed`, and closes with `authorization_revoked`. | fixed | `aab2cf3` |
| R-12 | Medium | Access control / sessions | `crates/yanto-server/src/stream_guard.rs:115` | The deployment log/event SSE streams, the container log SSE streams (dashboard and REST) and the app log SSE stream never re-checked access. Fix: `revocable` re-authenticates the cookie or token every 15 s, re-runs the read check, and ends with `authorization_revoked`. | fixed | `9691cdd` |
| R-13 | High | Host-file writers (availability) | `crates/yanto-host/src/cron.rs:170`, `crates/yanto-host/src/traefik.rs:65` | The unit has `ProtectSystem=strict` with `ReadWritePaths` limited to `/etc/cron.d` and the Traefik dynamic dir. Yet the cron writer staged `/etc/.yanto-cron.tmp`, and the Traefik writer staged its temp and backup files in the dynamic dir's *parent*. Every cron edit and every Traefik Hostname change would fail with EROFS/EXDEV in production; tests passed because they run without the sandbox. Fix: staging moved inside the writable dir, using names cron and Traefik ignore. 04 §5.4 and §10 were updated. | fixed | `4870acc` |
| R-14 | Low | Command injection (option injection) | `crates/yanto-core/src/service/project_databases.rs:69` | Database and service names could start with `-`; for example `-r<file>` became a `mysqldump` option. A leading `-` or `.` is now rejected. | fixed | `b82d825` |
| R-15 | Low | Secrets management / SSRF | `crates/yanto-host/src/frp.rs:87` | The frps dashboard client honored `HTTP(S)_PROXY`, so Basic-auth credentials could go to a proxy. It now uses `no_proxy()`. | fixed | `bcdf22a` |
| R-16 | Low | Rate limiting | `crates/yanto-core/src/service/deployments.rs:1336` | The GitHub webhook limit was keyed by Project only, so unsigned floods blocked GitHub's real deliveries. It is now keyed by IP + Project (03 a.4). | fixed | `0a87759` |
| R-17 | Low | Correctness | `crates/yanto-core/src/db/repo/deployments.rs:400` | Each flush past the log cap added another truncation marker that was never removed, and `log_bytes` ignored system rows. Now there is one marker and the byte count is honest. | fixed | `73d3921` |
| R-18 | Low | Correctness | `crates/yanto-core/src/deploy/runner.rs:97` | When the project was missing, `execute`'s early return leaked the cancel token. A DB error on that path was also reported as "project no longer exists". | fixed | `c8e0151` |
| R-19 | Medium | Correctness (concurrency) | `crates/yanto-core/src/service/projects.rs:751` | Project delete checked for running Deployments on a reader and then ran `compose down [--volumes]` for up to 300 s. A queued Deployment claimed in that window brought the stack back up mid-delete. Fix: the running check and cancelling queued Deployments now happen in one write transaction before `down` (partial fix: see N-09). | fixed | `7e18b39` |
| R-20 | Medium | Half-merged code | `crates/yanto-cli/src/ops/uninstall.rs:191` | `--stop-projects` and `--remove-registry` were still `TODO(M2)`/`TODO(M6)` stubs, so `--purge` left orphan stacks and the registry running. They now run `docker compose -p <slug> down` per Project and `docker rm -f yanto-registry`. | fixed | `44f3e7f` |
| R-21 | Low | Quality (misleading name, duplicate validator) | `crates/yanto-core/src/service/backups.rs:192` | `valid_slug` guarded slugs *and* volume/service target names, using rules that differ from `validate::slug`. It is renamed `safe_component` and now also rejects a leading `-`. | fixed | `b82d825` |
| R-22 | Low | Correctness | `crates/yanto-core/src/deploy/worker.rs:25` | The 6 s kill grace was shorter than one command's 5 s SIGTERM grace plus 2 s + 2 s reader waits. Runners were aborted before they could finalize, leaving `interrupted` instead of `cancelled`. Raised to 12 s, which stays within `TimeoutStopSec=45`. | fixed | `2e6e953` |

A formatting-only commit follows the fixes: `3bcaf9d style: cargo fmt`.

## Recorded, not fixed

| Id | Sev | Risk area | Location | Finding | Recommendation |
|---|---|---|---|---|---|
| N-01 | High (design) | Access control / privilege boundary | `crates/yanto-core/src/deploy/compose.rs:26` | Compose content is not restricted: `privileged`, `/` or `/var/run/docker.sock` bind mounts, `pid: host` and `network_mode: host` are all accepted. Any of the following can therefore become root on the Host: a Member with `config` (sets `compose_content` or `git_url`) plus `deploy`; anyone who can push to a Project's repo when webhooks are on; an Owner `write` token. 04 §1 calls `deploy` root-equivalent, but per-Project grants read as isolation in 03 a.2. | Choose one. (a) State in 03 a.2 and the UI that `config`+`deploy` = Host root, and grant them only to trusted people. (b) Add a compose policy enforced on the `docker compose config --format json` output (deny `privileged`, host namespaces, `cap_add`, `devices`, bind mounts outside the Project dir) with an Owner-only per-Project override. (b) is a product decision and changes behaviour, so it was not made here. |
| N-02 | Medium (design) | Access control | `crates/yanto-core/src/service/projects.rs:544` | A Member with `config` may attach *any* existing Deploy Key or Registry Credential (03 a.2: "select among existing ones"). Combined with `git_url`/`image_ref` changes and `deploy`, this lets them clone another Project's private repo or pull private images with the Owner's credentials. | Scope credentials per Project (an allow-list on the credential), or require the Owner to assign them. Needs an amendment to 03 a.2. |
| N-03 | Low | Access control (information disclosure) | `crates/yanto-core/src/service/deploy_keys.rs:17`, `crates/yanto-core/src/service/registry_credentials.rs:16` | `deploy_key_list`, `deploy_key_public_get` and `registry_credential_list` are open to any authenticated actor, including a Member's `read` token. They expose names, hosts, usernames and public keys. 03 a.2 puts `deploy_key_public_get` under `secrets`. `hostnames_options` also shows tunnel and zone names to Members without `hostnames`. | Add an `Access` variant "Owner or any grant with permission P" (`config` for the lists, `secrets` for public get, `hostnames` for options). It is left as is because the Member `config` UI depends on these lists. |
| N-04 | Low | Access control (scope) | `crates/yanto-core/src/service/registry.rs:47` | `registry_user_password_set` needs only `write` scope, while 03 a.5 puts credential updates at `admin`. The operation is Owner-only either way. | Raise it to `Scope::Admin` and update the MCP snapshot. |
| N-05 | Low | Access control (existence oracle) | `crates/yanto-core/src/service/backups.rs:803` | `delete` and `download` load the row before authorizing, so 403 and 404 differ for ids the caller cannot see. Ids are 96-bit random, so impact is negligible. | Authorize the scope first, then map "not visible" to 404, as `hostnames::scope_guard` does. |
| N-06 | Low | Availability / process cleanup | `crates/yanto-host/src/project_backup.rs:106` | The `docker exec`/`docker run` calls for dumps and restores have no timeout and no process group. `StartGuard::drop` runs a blocking `docker compose start`. `systemctl` calls in `host/cron.rs` and `cli/cmd/backup.rs` are synchronous with no timeout. | Route them through `CommandPort` (timeouts, kill-on-drop, process group). |
| N-07 | Low | Install/self-update verification | `crates/yanto-cli/src/cmd/self_update.rs:170` | Without `gh`, `SHA256SUMS` comes from the same release, so it proves integrity but not authenticity (this is how D19/D27 are written). `install.sh`'s `wget --https-only` probably affects only recursive downloads (unverified). | Ship a signature that does not depend on `gh` (minisign/cosign key pinned in the binary) in a later release. Test `install.sh` against a downgrade-to-http redirect. |
| N-08 | Low | Correctness | `crates/yanto-core/src/deploy/runner.rs:234` | If the finalize transaction fails (for example after the 5 s busy timeout), the error is only logged. The row stays `running`, so the Project's queue is blocked until restart recovery runs. | Retry `finish` with backoff. On final failure, publish `Done` and unregister anyway. |
| N-09 | Low | Correctness (concurrency) | `crates/yanto-core/src/service/projects.rs:751` | Remainder of R-19: a *new* trigger that arrives during `compose down` can still be queued and claimed. | Add a `deleting` flag on the Project that `enqueue` refuses and `claim_next` skips, or delete the rows first and run `down` afterwards. |
| N-10 | Low | Access control (streams) | `crates/yanto-core/src/service/deployments.rs:681` | `events_stream` captures the actor's visible-Project set at open. R-12 ends the stream when the session or token is revoked, but a *removed grant* keeps status events for that Project flowing until reconnect. These are status-only events, not logs. | Re-filter from a freshly loaded actor on each recheck (pass a watch channel into the service). |
| N-11 | Low | Quality (duplication) | `crates/yanto-core/src/deploy/files.rs:56`, `crates/yanto-host/src/fsutil.rs:30`, `crates/yanto-cli/src/ops/install.rs:43`; `crates/yanto-host/src/commands.rs:103`, `crates/yanto-host/src/cron_exec.rs` | Three atomic-write helpers with different guarantees. Only `fsutil` has `O_NOFOLLOW`, dir fsync and `fchown`; the CLI one uses a fixed temp name. Two separate SIGTERM→SIGKILL process-group killers with separate constants. These are leftovers from parallel milestones. | Move `fsutil::atomic_write` and `DirHandle` into one shared place (core has no `nix` dependency, so a small shared crate or a re-export from host), and have core and CLI call it. Share one `terminate_group` with one set of constants. |
| N-12 | Info | Command injection (config-controlled) | `crates/yanto-core/src/deploy/runner.rs:273` | `GIT_SSH_COMMAND` is a shell string that includes `data_dir`. ssh runs it through `sh -c`, so a `data_dir` with spaces or shell characters would break or inject. Only root sets that value (`config.toml`). | Quote the paths with shell escaping, or validate that `data_dir` is a plain absolute path at config load. |
| N-13 | Info | Path traversal (TOCTOU) | `crates/yanto-core/src/deploy/files.rs:99` | R-01 checks the canonical path, then opens by path. The checkout only changes during a Deployment, which holds the Project's single running slot, but a container bind-mounting the Project dir could still race the check. | For full robustness, open the Project dir as a `DirHandle` and resolve components with `openat(O_NOFOLLOW)` (Linux `openat2(RESOLVE_BENEATH)`). |
| N-14 | Info | Verification gap | 07 OQ-26, `crates/yanto-host/src/cron.rs:170` | R-13 moves the cron temp file into `/etc/cron.d` as `.yanto-cron.tmp`. Debian cron skips dotted names, and cronie skips dot-files (unverified on a live host). No test runs the writers under the real unit's `ProtectSystem=strict`. | Add a CI job on Debian 12 and Ubuntu 22.04 that installs the unit and drives a cron edit, a Traefik Hostname and an Authorized Key through the running service. |

## Checked and found sound (summary)

- **Authorization core (`actor.rs`):**
  - Deny-by-default.
  - Session-only and scope checks run before role checks.
  - Missing grant returns 404, missing permission 403.
  - D23 capping holds.
  - Token issuance, listing and revocation are session-only.
  - Exec is `session_only` and not routed in REST/MCP (D24).
  - MCP `tools/list` hides tools above the token's scope.
- **IDOR:** deployments are authorized against the row's own Project. Containers map to Projects by the Compose label, and unlabeled containers are Owner-only. Hostnames, backups and volume backups check the row's Project.
- **Secrets:**
  - AES-256-GCM envelope with a random nonce and AAD `table.column.id`; `app.key` is 0600 and refused when loose.
  - Tokens, sessions, the setup code and the Deploy Token are stored hashed and compared in constant time.
  - GitHub HMAC is computed over the raw body, `sha256=` only.
  - `Secret` redacts both `Debug` and `Display`.
  - No secrets in argv (per-command `DOCKER_CONFIG`, bcrypt computed in-process).
  - Audit metadata records env key names only.
- **Command construction:** argv only; `git clone … -- <url> .`; leading `-` is rejected for URLs, refs, images and paths; `GIT_ALLOW_PROTOCOL=ssh:https`. The cron file holds only `cron-run <id>`, and `exec-as` drops privileges in the order `initgroups`, `setgid`, `setuid`, then verifies.
- **Backup archives:** Yanto restore accepts exactly two regular entries. Volume restore runs in a `--network none` helper container.
- **Migrations:** 0201/0550 are correct rebuilds under `defer_foreign_keys`, with indexes and triggers recreated and `cron_runs` parked so the drop cannot cascade. Every connection sets `foreign_keys`, `busy_timeout` and WAL.

## Verification (after all commits)

| Check | Result |
|---|---|
| `cargo fmt --check` | clean |
| `cargo clippy --workspace --all-targets --locked -- -D warnings` | clean |
| `cargo test --workspace --locked` | 570 passed, 0 failed, 5 ignored (Docker-only) |
| `cargo test -p yanto-host -- --ignored` (local Docker) | 4 passed (exec/logs lifecycle, git+image deploy/rollback, volume+DB backups, registry); no containers or volumes left behind |
| `mise run audit` | exit 0; 1 allowed warning (RUSTSEC-2026-0543, `chrono` unmaintained, informational) |
| `gitleaks git --no-banner --redact` | no leaks found |
