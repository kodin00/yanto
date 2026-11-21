> Classification: INTERNAL

# M2 deviations and open items (Projects & Deployments)

Recorded by M2 in the style of [08-conventions](08-conventions.md) section 9. Behavior and data decisions stay in [00-scope](00-scope.md).

## Deviations from 00-07

- **Docker/Git/Compose all go through the `docker` and `git` CLIs** behind one `CommandPort` (`yanto-core::ports::command`, real impl `yanto-host::commands`), not `bollard`. M2 needs only `docker pull`, `docker ps`, `docker image inspect`, `docker manifest inspect` and `docker compose`; `bollard` arrives with M3 (container list, logs, exec) and can replace the `docker ps` call in `deploy::portcheck` later.
- **Ports are derived, then reconciled.** `project_ports` is rewritten from `docker compose config --format json` on save (when the compose text is pasted) and on every Deployment; only a non-loopback publish or a collision fails the Deployment (04 1.3 step 7 said a mismatch fails it, which would block the first deploy). `project_ports_set` also exists for manual declarations. No YAML fallback parser: when `docker compose config` cannot answer at save time, port derivation is skipped until the first Deployment.
- **Collision rule (d)** reads `/proc/net/{tcp,tcp6,udp,udp6}` and maps sockets to a process name through `/proc/<pid>/fd`; on non-Linux it reports nothing. Ports published by the Project's own running containers are excluded from the host-listener rule so a redeploy never collides with itself.
- **Deploy Key import/generate is ed25519 only** (`ssh-key` with `ed25519`; no RSA/ECDSA, which avoids the `rsa` crate).
- **Image rollback** restores the exact compose snapshot, pulls `repo@sha256:<digest>` and re-tags it as the Project's `image_ref` instead of generating an override (works for multi-service compose files). Git rollback to a Deployment checks out its `commit_sha`; an explicit `git_ref` rollback has no config snapshot.
- **Rollback preview** does not run `git fetch --all --tags` (it is read-only and uses commits already present locally).
- **The Deploy Token reveal** is `admin` scope with the `secrets` permission (03 a.5); REST `env/content?reveal=true` also needs `admin` and is audited as `project.env_read`.
- **Webhook routes** (`/deploy`, `/webhooks/github`) are mounted in `app.rs`, not in `dashboard::auth::public_routes`: they accept no cookie, are outside the CSRF/Origin guard (GitHub sends no `Origin`), and authenticate in the service (`deploy_webhook`, `github_webhook` are `Access::Public` operations with matrix rows).
- **Registry auth directory** is `/run/yanto/docker-<deployment id>/` when `/run/yanto` exists (systemd `RuntimeDirectory`), else `<data_dir>/run/`. Stale `docker-*` directories are swept when `yanto serve` starts.
- **Test fixtures** `TestCore::create_member` now creates a stub Project row for every granted id (`ensure_project`), because grants reference `projects` since 0201 and `user_access_update` / `user_create` reject unknown ids with 422.
- **Log colors**: ANSI SGR is mapped to `ansi-N` classes (30-37, 90-97, bold); every other escape is dropped. The dashboard uses a small `EventSource` script (`assets/js/deployment-log.js`) instead of the htmx SSE extension (not vendored).

## Not done in M2

- The audited "Reset checkout to origin/<branch>" action offered after a non-fast-forward pull (04 1.6) and `yanto deploy` on the CLI.
- Project `access`, `hostnames`, `databases` and `backups` tabs (other milestones). The Users page still takes a Project id as text.
- Registry login probe on credential save (04 4.7, "reported but not blocking") and Hosted Registry credentials (M6).
- Checks that a declared port differs from `frps` ports and the Hosted Registry port; running listeners cover them while those services are up.

## Unverified

- An end-to-end run against a real Docker daemon: `crates/yanto-host/tests/docker_smoke.rs` (`#[ignore]`) was written but could not be completed because the daemon was not running when M2 was built.
- `docker compose config --format json` output shape and `--ignore-buildable` support on the Compose v2 minimum version (OQ-22); `cli-plugins` discovery under a temporary `DOCKER_CONFIG` relies on the `cli-plugins` symlink.
