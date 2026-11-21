> Classification: INTERNAL

# 07 — Open questions

Deduplicated from the open-question and risk sections of docs 01–05, from [06-milestones](06-milestones.md), and from ASSUMPTION items that need an owner decision. Pure implementation-time verification items are kept only where they can change a design (marked "verify"). "Blocks" is the first milestone that cannot finish without the answer. Ids are stable: other docs cite them.

## Open

| Id | Question | Referenced in | Recommended answer | Blocks |
|---|---|---|---|---|
| OQ-1 | Deploy Token must be hashed, revealable, and the GitHub HMAC key. Keep the hash + encrypted copy design, or split a separate webhook secret (bearer hash-only, reveal = rotate-and-show)? | 01 §5.4/§8.3, 02 §3/§7, 03 §a.6/§Open, 04 §1.7 | Keep hash + `deploy_token_enc` (legacy parity, one secret to manage); reveal stays `secrets`-gated, audited, `admin` + `confirm` over MCP. | M2 |
| OQ-2 | `rmcp` is pre-1.0 and its version and feature flags are unverified. What is the go/no-go criterion for dropping MCP over HTTP (ADR 0005) and keeping stdio? | 01 §4.1/§12, 03 §d, 06 risks, ADR 0005 | Pin exactly; time-box a spike at M8 start; drop HTTP if the spike needs more than 2 days or Origin/auth middleware cannot be mounted; v1.0.0 ships with stdio only in that case. | M8 |
| OQ-3 | Should `yanto install` ever install Docker/Compose/Git (opt-in `--install-prereqs`), or only print commands? Package names in the hints are unverified. | 05 §1/§3.1/§11 | Print only (consistent with D15/D19); verify package names against Debian 12 and Ubuntu 22.04 before shipping the text. | M4 |
| OQ-4 | Setup code lifetime and length (D18 says only "one-time"). | 05 §3.6/§11, 03 §a.3 | 16 characters (~80 bits), valid until used or 24 h, re-issuable with `yanto setup-code` while no Owner exists. | M1 |
| OQ-5 | Remote push to the Hosted Registry through a Cloudflare Tunnel may hit request-body limits (limit unverified). Which path is the documented default? | 04 §4.5/§4.8, 06 risks | Document Traefik as the recommended remote-push path, Cloudflare Tunnel as "small layers only", and verify the limit against current Cloudflare docs before writing a number; test a large image in M6. | M6 |
| OQ-6 | Dashboard sessions are 7 days absolute with no idle timeout (legacy parity). Add an idle timeout for internet-reachable dashboards? | 03 §a.3/§Open 5, 01 §5.2 | Add a configurable idle timeout (default 24 h) using the existing `last_seen_at`; keep the 7-day absolute cap. | M1 |
| OQ-7 | In-memory rate limits reset on restart; repeated restarts weaken brute-force protection. Persist them? | 03 §a.4/§Open 6 | Accept for v1.0.0 (loopback-first, restarts are rare); revisit if the dashboard is commonly exposed. | M1 |
| OQ-8 | New behaviors not in the parity checklist: `deployment_cancel`, `backup_restore`, `cloudflare_tunnel_activate`, per-session CSRF token, declared Project databases, `yanto deploy`/`doctor`. Add them to 00-scope as **New** items? | 03 §Open 4, 00-scope checklist | Yes: add them as **New** items so milestones can tick them. | M2 (cancel), M5 (restore), M7 (activate) |
| OQ-9 | CONTEXT.md wording no longer matches the plan: Hostname may target the Hosted Registry (`target_kind='registry'`); a Cron Entry's cron-file user field is `root` with the real user in `run_as_user` (D26); the Hosted Registry entry says "runs on the Master" (a multi-node term). Amend the glossary? | 02 §4.6, 04 §4.5/§5.3, CONTEXT.md | Yes: amend CONTEXT.md (Hostname: "Project's published port, or the Hosted Registry"; Cron Entry: "runs as a named Unix user via `yanto cron-run`"; Hosted Registry: "runs on the Host"). | M5 |
| OQ-10 | Self-update rollback after a successful forward-only migration needs a manual DB restore. Acceptable for v1.0.0? | 05 §7/§11 | Accept; `--rollback` prints the restore steps and the pre-update DB copy stays under `backups/yanto/`. | M4 |
| OQ-11 | systemd hardening (Tier B: `ProtectSystem=strict` + generated `ReadWritePaths`) is unverified, and operator-changed paths silently invalidate `ReadWritePaths`. | 05 §5/§11 | Ship Tier A + B by default, `--hardened` off; add a startup self-check that names any configured path missing from `ReadWritePaths`; run the Docker, cron, Traefik and `authorized_keys` suites on Debian 12 and Ubuntu 22.04 and record `systemd-analyze security`. | M4 |
| OQ-12 | mise: is the `cargo:` backend usable, which tool versions to pin, and is `mise.lock` committed? | 05 §4.1/§11 | Commit `mise.lock`; pin exact Rust and tool versions at the first commit after checking each. | M0 |
| OQ-13 | Native arm64 CI runner label and musl + bundled SQLite build details are unverified. | 05 §9.1/§11 | Use a native arm64 hosted runner if available; fallback `cargo-zigbuild`; prove both builds in M0 CI. | M0 |
| OQ-14 | `curl \| bash` + interactive wizard depends on `/dev/tty`; behavior over non-TTY SSH and cloud-init is untested. | 05 §2/§11 | Keep the `--non-interactive` path first-class and test both paths in the install smoke job. | M4 |
| OQ-15 | `self-update` uses the GitHub API (unauthenticated rate limit unverified). | 05 §7/§11 | Use the `releases/latest/download` redirect as the no-API fallback; honor optional `GITHUB_TOKEN`. | M4 |
| OQ-16 | `yanto mcp` (stdio) runs in-process against the shared SQLite DB, so Deployments it enqueues rely on the `serve` poll fallback. Proxy to the local REST API instead? | 01 §4.1/§12 | Keep in-process (no token round trip, works with `serve` down); Deployments wait queued until `serve` runs. | M8 |
| OQ-17 | D23 says tokens belong to a user; may Members create their own API Tokens? (Docs currently assume yes, own tokens only, capped by their permissions.) | 03 §a.5/§b, 02 §4.1 | Yes, in the dashboard only, for themselves; Owner can list and revoke all tokens. | M1 |
| OQ-18 | SQLite/sqlx engine details are unverified: single-writer pool starvation, `STRICT`/`json_valid` need SQLite >= 3.37 in the bundled build, cross-table triggers for Hostname/port consistency, `VACUUM INTO` stalling writers, bcrypt cost vs registry latency. | 01 §6.4/§12, 02 §1/§7 | Verify bundled SQLite version in M0; keep 1 writer + N readers; run `VACUUM INTO` off-peak; add trigger tests in M2/M7. | M0 |
| OQ-19 | Library versions and APIs are unverified: `rmcp` features, `bollard` (exec resize name), `axum`, `askama`, `sqlx`, `clap`, `tower-sessions`, Rust edition 2021 vs 2024. | 01 §2/§11, 03 §d, 04 §2.2 | Verify on crates.io/docs at the first `cargo add` per crate and pin in `[workspace.dependencies]`; edition 2024 if the pinned toolchain supports it. | M0 |
| OQ-20 | Vendored JS (htmx, SSE extension, xterm.js) needs an update procedure and checksum list to honor "no npm". | 01 §12 | `assets/VENDOR.md` with source URL, version, SHA-256, and an update script; CI checks the checksums. | M0 |
| OQ-21 | `registry:2` tag semantics and the in-container config path (Distribution 2.x vs later). | 04 §4.1/§4.2 | Pin by digest after first pull (stored in `hosted_registry_settings.image_digest`); pass the config path explicitly as the container command. | M6 |
| OQ-22 | `docker compose config --format json` stability for port derivation, and `cli-plugins` discovery under a temporary `DOCKER_CONFIG`. | 04 §1.1/§4.7 | Test both on Compose v2 minimum version in the Docker-dependent CI job; fall back to YAML parsing and a symlinked plugin dir. | M2 |
| OQ-23 | `frps` dashboard API response shapes and `frps verify` existence are unverified. | 04 §8 | Verify against the pinned frp release; skip `verify` if absent; show raw fields where shapes differ. | M7 |
| OQ-24 | Volume Backups need a helper image (`busybox:stable`) that may not be available offline. | 01 §12, 04 §7.2/§7.8 | Pull once on first use with a clear error when offline; allow `backup.helper_image` override; pin by digest. | M5 |
| OQ-25 | Exec terminal: lingering processes after hangup, and optional session recording. | 01 §12, 04 §2.2/§2.5 | No recording in v1; warn in Audit when `inspect_exec.running` stays true 10 s after close; idle timeout 15 min. | M3 |
| OQ-26 | Cron temp-file naming (dotted names skipped by Debian cron) and rescan behavior need a real test. | 01 §7.3/§12, 04 §5.2/§5.4 | Use `/etc/.yanto-cron.tmp`; verify on Debian 12 and Ubuntu 22.04 in the host-file integration job. | M5 |
| OQ-27 | Minimum Cloudflare API token permissions and tunnel `status` values are unverified. | 04 §9.2 | Verify against Cloudflare docs; ship the permission checklist in the client form. | M7 |

## Resolved by owner decisions

| Id | Question | Resolved → | Where applied |
|---|---|---|---|
| OQ-28 | Secrets key location and what a Yanto DB Backup contains | D22 | 01 §8.3, 02 §3, 04 §7 |
| OQ-29 | API Tokens: Project scoping | D23 | 02 §4.1, 03 §a.1/§a.5 |
| OQ-30 | REST exec for admin tokens | D24 | 03 §a.2/§c/§d, 04 §2.5 |
| OQ-31 | Rollback does not version compose/env | D25 | 02 §4.2, 04 §1.1 |
| OQ-32 | `cron-run` privilege-drop wrapper vs daemon socket; run history | D26 | 01 §7.3, 02 §4.4, 04 §5.3 |
| OQ-33 | Release authenticity beyond SHA256 | D27 | 05 §2/§9.2 |
| OQ-34 | `frps` apply restarts and drops tunnels | D28 | 04 §8.4 |
