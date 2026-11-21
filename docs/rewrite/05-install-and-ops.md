> Classification: INTERNAL

# 05 — Install, operations, build, and release

Binding inputs: [`CONTEXT.md`](../../CONTEXT.md), [`docs/adr/`](../adr/), [`00-scope.md`](00-scope.md) (D6, D7, D15, D18–D21). Architecture context: [`01-architecture.md`](01-architecture.md).

Conventions: **ASSUMPTION** = not grounded in provided docs, legacy code, or library docs. Versions are written only if shown by docs; otherwise **unverified**. The upstream repo path `github.com/kodin00/yanto` is taken from legacy `scripts/install.sh`.

## 1. Summary

| Topic | Decision |
|---|---|
| Install entry | `curl -fsSL …/install.sh \| sudo bash` → downloads release binary, verifies SHA256, `exec yanto install` |
| Wizard | `yanto install`: prereq checks, prompts (each has a flag), writes config + unit, starts service, prints a one-time setup code |
| Service | systemd, root, hardened where it cannot break host management (§5) |
| Update | `yanto self-update` (release path) with automatic DB backup and rollback |
| Build | `mise` pins Rust + tools; `cargo build --release --target …-musl` |
| Release | GitHub Actions on tag: musl x86_64 + aarch64 binaries, `SHA256SUMS`, GitHub Release. **No GHCR image.** |
| Wizard installs prerequisites? | **No** — checks and prints the exact commands (ASSUMPTION; legacy script apt-installed Docker; see open question 1) |

## 2. `install.sh`

Thin POSIX-ish bash (`#!/usr/bin/env bash`, `set -euo pipefail`, `umask 022`). Served as a Release asset (`releases/latest/download/install.sh`) and kept in `scripts/install.sh` in the repo. It contains no product logic beyond fetch + verify.

Steps:
1. **Preconditions**: Linux only (`uname -s`); root, else re-exec with `sudo` if available, else fail with a message. Requires `curl` or `wget`, `sha256sum`, `uname`, `install`. Fails with a clear message if missing; it does not install them.
2. **Arch detect** (`uname -m`): `x86_64|amd64` → `x86_64-unknown-linux-musl`; `aarch64|arm64` → `aarch64-unknown-linux-musl`; anything else → error listing supported arches (D20).
3. **Resolve version**: default = latest Release (`https://github.com/kodin00/yanto/releases/latest/download/<asset>`; pre-release tags are marked prerelease so `latest` skips them). Override with `YANTO_VERSION=v1.0.0` → `…/releases/download/v1.0.0/<asset>`. Also `YANTO_REPO` to override `owner/repo` (forks, testing).
4. **Download** to a fresh `mktemp -d` directory (mode 0700): asset `yanto-<target>` and `SHA256SUMS`. Use `curl -fsSL --proto '=https' --tlsv1.2 --retry 3`.
5. **Verify SHA256**: `grep " yanto-<target>$" SHA256SUMS | sha256sum -c -`; abort and delete temp dir on mismatch or missing line. Note: the checksum file comes from the same Release, so this proves integrity (corruption, truncated download) but **not** authenticity if the Release itself is compromised. Mitigation path is open question 2 (signature/attestation).
6. **Install binary**: `install -m 0755 -o root -g root <tmp>/yanto-<target> /usr/local/bin/yanto` (via a temp name in the same dir + `mv` for atomic replace).
7. **Hand off**: `exec /usr/local/bin/yanto install "$@"` — every script argument is passed through to the wizard. If stdin is not a TTY (the `curl | bash` case) and the wizard is interactive, reopen the terminal: `exec … </dev/tty` when `/dev/tty` is readable; otherwise the wizard fails with "pass `--non-interactive`".
8. Prints the installed version and exits non-zero on any failure; leaves nothing in `/etc` (all config writing is in `yanto install`).

`install.sh` never edits system files other than `/usr/local/bin/yanto`, never starts a service, never touches Docker.

Upgrade through `install.sh` is supported (re-running replaces the binary and re-enters `yanto install`, which detects an existing install, §3.5), but the documented path is `yanto self-update`.

## 3. `yanto install`

Runs as root (refuses otherwise). Used identically by the `install.sh` path and by self-built binaries (D19).

### 3.1 Prerequisite checks

Run first, before any prompt; results are printed as a table (`ok / warn / fail`). `fail` aborts (unless `--skip-prereq-checks`, discouraged — ASSUMPTION), `warn` continues.

| Check | How | Fail/warn |
|---|---|---|
| Root | `geteuid() == 0` | fail |
| Linux + systemd | `/run/systemd/system` exists; `systemctl --version` | fail |
| Distro support | `/etc/os-release`: Debian ≥ 12, Ubuntu ≥ 22.04 are tested (D20) | warn on others |
| Docker engine | `docker` on PATH **and** daemon reachable on the configured socket (bollard ping or `docker info`) | fail; print install hint |
| Compose plugin | `docker compose version` succeeds (plugin, not legacy `docker-compose`) | fail; print hint |
| Git | `git --version` | fail; print hint (required for Git-sourced Projects, D9) |
| Port free | the chosen bind address:port is not already listening | fail (prompt re-asks) |
| Disk space on data dir's filesystem | ≥ 2 GiB free (ASSUMPTION) | warn |
| Existing install | `/etc/yanto/config.toml` or unit present → §3.5 | n/a |

Install hints print the distro's own documented commands (Debian/Ubuntu: Docker's official apt repo instructions or `docker.io` + `docker-compose-v2` packages — package names **unverified**, must be confirmed before shipping the text). Yanto does not run them.

### 3.2 Prompts and flags

Every prompt has a flag; a flag given on the command line suppresses its prompt. `--non-interactive` (alias `--yes`) never prompts: it uses flag values, then defaults, and fails if a value has no default and wasn't supplied.

| Prompt | Flag | Default | Validation / notes |
|---|---|---|---|
| Config file path | `--config <path>` | `/etc/yanto/config.toml` | absolute path |
| Bind address | `--bind <addr>` | `127.0.0.1` | valid IP; non-loopback prints a warning that the dashboard is then plain HTTP on that interface (put it behind an Ingress Provider instead); in non-interactive mode a non-loopback bind requires `--allow-public-bind` (ASSUMPTION) |
| Port | `--port <n>` | `9197` | 1–65535, not in use |
| Public URL (optional) | `--public-url <url>` | empty | `http(s)://…`; drives cookie `Secure` + CSRF origin check (see 01 §5) |
| Data dir | `--data-dir <path>` | `/var/lib/yanto` | absolute; created `0700 root:root` |
| Unix user for Authorized Keys | `--ssh-user <name>` | `$SUDO_USER` if set and not root, else first non-system user (uid ≥ 1000) | must exist in `/etc/passwd`; `root` allowed but warns. Non-interactive with no derivable default → error requiring `--ssh-user` |
| Enable Hosted Registry | `--enable-registry` / `--no-enable-registry` | no | if yes: verifies Docker can pull `registry:2` lazily at first enable in the dashboard (not during install) and port `127.0.0.1:5000` is free |
| Ingress Providers to expect | `--ingress <list>` (`cloudflare-tunnel`, `traefik`, `none`; comma-separated or repeated) | `none` | multi-select. For each selected provider the wizard runs the daemon check (§3.3) |
| Traefik dynamic dir | `--traefik-dynamic-dir <path>` | `/etc/traefik/dynamic` (ASSUMPTION) | asked only if `traefik` selected; dir should exist (warn if not) |
| Traefik cert resolver | `--traefik-cert-resolver <name>` | `letsencrypt` (ASSUMPTION) | asked only if `traefik` selected |
| Expect `frps` | `--frps` / `--no-frps` | no | if yes: daemon check + `--frps-config <path>` (default `/etc/frp/frps.toml`, ASSUMPTION) |
| Start service now | `--start` / `--no-start` | start | `--no-start` writes everything but does not `enable --now` |
| Reconfigure existing install | `--reconfigure` | off | §3.5 |
| Release channel/version for later self-update | — | n/a | not a wizard concern |

Global: `--log-level`, `--no-color`. Defaults shown in the prompt text; secrets are never prompted (none are needed at install).

### 3.3 Host Daemon checks

For each expected Host Daemon (`cloudflared`, `frps`, Traefik) report: **binary found** (PATH lookup), **unit exists** (`systemctl cat <unit>`), **enabled**, **active**. Unit names default to `cloudflared`, `frps`, `traefik` and are configurable (`--cloudflared-unit`, `--frps-unit`, `--traefik-unit`, ASSUMPTION). A missing/inactive daemon is a **warning, not a failure** — the operator may install it later; the dashboard shows the same check and install instructions. Yanto never installs a Host Daemon (D15, ADR 0006).

### 3.4 What it writes (in order, each step idempotent)

1. Create data dir tree with modes from [01 §8.2](01-architecture.md); generate `keys/app.key` (32 random bytes, `0600`) **only if absent** (never regenerated: doing so would orphan encrypted secrets).
2. Write `config.toml` (`root:root 0600`, atomic temp+rename), including `config_version = 1` (ASSUMPTION; add to schema in 01).
3. Run DB migrations against `yanto.db`.
4. Generate a **setup code** (§3.6) and store only its hash in the DB.
5. Write `/etc/systemd/system/yanto.service` (§5) rendered from config (paths, `ReadWritePaths`), `systemctl daemon-reload`.
6. `systemctl enable --now yanto` (unless `--no-start`), then poll `GET http://<bind>:<port>/healthz` up to 30 s; on failure print `journalctl -u yanto -n 50` hint and exit non-zero (config and unit remain for debugging).
7. Print summary: URL, config path, unit name, and the setup code.

It does **not** write `/etc/cron.d/yanto` or any `authorized_keys` content at install time (those appear when the Owner creates entries), and does not touch Traefik/frps/cloudflared configuration.

### 3.5 Re-run / existing install

- Detected by existing config or unit. Interactive: asks "Reconfigure? Upgrade only? Abort?". Non-interactive: refuses unless `--reconfigure` or `--force`.
- Reconfigure reuses current values as prompt defaults, rewrites config + unit, keeps data dir, DB, and `app.key`, restarts the unit.
- Binary upgrade only: use `yanto self-update` (§7); `yanto install` does not replace binaries when invoked from a self-built path.
- If an Owner already exists, no new setup code is printed (use `yanto setup-code` only while no Owner exists, or `yanto owner-reset`).

### 3.6 One-time setup code

- Random, 16 chars from an unambiguous alphabet, shown grouped (`XXXX-XXXX-XXXX-XXXX`); entropy ≈ 80 bits (ASSUMPTION on length).
- Stored hashed (SHA-256, constant-time compare). Valid until used **or** 24 h (ASSUMPTION; no expiry stated in D18), consumed atomically with Owner creation.
- Printed **once** on stdout at the end of install, never logged by the service, never written to disk in plaintext. `yanto setup-code` issues a fresh one (revokes the old) while no Owner exists. It is a live credential until used (treat as RESTRICTED): the installer prints a warning to clear terminal scrollback and not paste it into tickets or chat.
- Dashboard `/setup` is reachable only while no Owner exists and requires this code (D18).

### 3.7 Install example (non-interactive)

```
sudo yanto install --non-interactive \
  --bind 127.0.0.1 --port 9197 --data-dir /var/lib/yanto \
  --ssh-user deploy --ingress cloudflare-tunnel,traefik --no-enable-registry
```

## 4. Building from source (mise + cargo)

Prereqs: `mise` installed, a C compiler (bundled SQLite via `libsqlite3-sys` — ASSUMPTION), `musl-tools` for the musl target on Debian/Ubuntu.

```sh
mise install                       # Rust toolchain + dev tools pinned in mise.toml
mise run build                     # cargo build --release --locked --target x86_64-unknown-linux-musl -p yanto-cli
sudo ./target/x86_64-unknown-linux-musl/release/yanto install
```
(Use `aarch64-unknown-linux-musl` on arm64 hosts; building natively on the target arch avoids cross toolchains.) No Node/npm anywhere (D3, D7). Dashboard assets (htmx, SSE extension, CSS, terminal JS) are vendored files in the repo and embedded at compile time.

Developer loop: `mise run dev` = `cargo run -p yanto-cli -- serve --config ./dev/config.toml` against a local `./dev-data` dir (host-file writers point at a temp root through config overrides so `/etc` is never touched; see 01 §10).

### 4.1 `mise.toml` (proposed)

Grounded: mise docs show `[tools] rust = { version, components, targets }` and `[tasks.<name>] run`/`depends`. The `cargo:` backend and all versions below are **unverified** — fill in real pins at first commit (`mise.lock` should be committed; mise-action docs describe a `mise.lock` for checksum verification).

```toml
[tools]
rust = { version = "<pin: unverified>", components = "rustfmt,clippy", targets = "x86_64-unknown-linux-musl,aarch64-unknown-linux-musl" }
"cargo:sqlx-cli"   = "<pin: unverified>"   # ASSUMPTION: cargo backend; features sqlite only
"cargo:cargo-audit" = "<pin: unverified>"
gitleaks           = "<pin: unverified>"

[env]
SQLX_OFFLINE = "true"                      # default to committed .sqlx metadata
# DATABASE_URL set only inside the `prepare` tasks below

[tasks.fmt]
run = "cargo fmt --all --check"
[tasks.clippy]
run = "cargo clippy --workspace --all-targets --locked -- -D warnings"
[tasks.test]
run = "cargo test --workspace --locked"
[tasks.test-docker]
description = "Docker-dependent integration tests (needs a local Docker daemon)"
run = "YANTO_DOCKER_TESTS=1 cargo test --workspace --locked -- --ignored"
[tasks.prepare]
description = "Regenerate .sqlx offline query metadata"
env = { SQLX_OFFLINE = "false", DATABASE_URL = "sqlite://target/prepare.db" }
run = ["sqlx database setup --source crates/yanto-core/migrations", "cargo sqlx prepare --workspace"]
[tasks."prepare-check"]
env = { SQLX_OFFLINE = "false", DATABASE_URL = "sqlite://target/prepare.db" }
run = ["sqlx database setup --source crates/yanto-core/migrations", "cargo sqlx prepare --workspace --check"]
[tasks.audit]
run = "cargo audit"
[tasks.secrets]
run = "gitleaks detect --source . --redact"
[tasks.build]
run = "cargo build --release --locked --target x86_64-unknown-linux-musl -p yanto-cli"
[tasks.ci]
depends = ["fmt", "clippy", "test", "prepare-check", "audit", "secrets"]
[tasks.dev]
run = "cargo run -p yanto-cli -- serve --config ./dev/config.toml"
```
sqlx-cli subcommand/flag spelling (`database setup --source`, `prepare --workspace --check`) is from sqlx-cli knowledge and **not re-verified** this session (ASSUMPTION). `gitleaks detect` vs newer subcommand names: unverified.

`[profile.release]` in the workspace `Cargo.toml` (ASSUMPTION): `lto = "thin"`, `codegen-units = 1`, `strip = "symbols"`, `opt-level = 3`, no `panic = "abort"` (01 §9).

## 5. systemd unit

`/etc/systemd/system/yanto.service`, rendered by `yanto install` (not a static file: `ReadWritePaths` depend on config).

```ini
[Unit]
Description=Yanto VPS management
Documentation=https://github.com/kodin00/yanto
After=network-online.target docker.service
Wants=network-online.target docker.service

[Service]
Type=simple
User=root
ExecStart=/usr/local/bin/yanto serve --config /etc/yanto/config.toml
Restart=on-failure
RestartSec=2
TimeoutStopSec=45
KillMode=control-group
UMask=0077
LimitNOFILE=65536
Environment=HOME=/root
Environment=DOCKER_CONFIG=/var/lib/yanto/docker
StateDirectory=yanto
StateDirectoryMode=0700

# --- sandboxing: Tier A (safe, always on) ---
NoNewPrivileges=yes
PrivateTmp=yes
ProtectKernelTunables=yes
ProtectKernelModules=yes
ProtectKernelLogs=yes
ProtectControlGroups=yes
ProtectClock=yes
ProtectHostname=yes
LockPersonality=yes
RestrictRealtime=yes
RestrictSUIDSGID=yes
SystemCallArchitectures=native
RestrictAddressFamilies=AF_UNIX AF_INET AF_INET6 AF_NETLINK

# --- Tier B (on; paths generated from config) ---
ProtectSystem=strict
ProtectHome=read-only
ReadWritePaths=/var/lib/yanto /etc/cron.d -/etc/traefik -/etc/frp -/home/deploy/.ssh

[Install]
WantedBy=multi-user.target
```

### 5.1 What is safe and what is not

Everything below is from systemd semantics as I know them (**ASSUMPTION**, not re-verified against systemd docs this session). The acceptance test is: run the full Docker-dependent suite and the host-file integration test on Debian 12 and Ubuntu 22.04 with the unit applied, then `systemd-analyze security yanto` for the exposure score.

**Tier A — safe, on by default** (none of these touch what Yanto manages):

| Option | Why it is safe here |
|---|---|
| `NoNewPrivileges` | Already root; no setuid helper needed. Children (`docker`, `git`, `ssh`, `systemctl`) don't require privilege escalation. |
| `PrivateTmp` | Yanto uses `<data_dir>/tmp`, never `/tmp`, for anything Docker/dockerd must see (the daemon lives outside this mount namespace and would not see a private `/tmp`). **Rule for code: never hand `/tmp` paths to Docker.** |
| `ProtectKernelTunables/Modules/Logs`, `ProtectClock`, `ProtectHostname`, `LockPersonality`, `RestrictRealtime`, `RestrictSUIDSGID`, `SystemCallArchitectures=native` | Yanto reads `/proc` and `/sys` (CPU/mem via `sysinfo`) but does not write them. |
| `ProtectControlGroups` | `/sys/fs/cgroup` read-only; Yanto does not manage cgroups (dockerd does). |
| `RestrictAddressFamilies=AF_UNIX AF_INET AF_INET6 AF_NETLINK` | Docker socket (UNIX), HTTP clients (INET*), `systemctl`/socket inspection (NETLINK). Do not drop `AF_NETLINK`. |

**Tier B — on, but only with generated config** (these are what keep host management working):

| Option | Constraint |
|---|---|
| `ProtectSystem=strict` | Makes the whole FS read-only except `ReadWritePaths`. The installer **must** list every path Yanto writes: data dir (`StateDirectory=` covers the default), `/etc/cron.d`, the Traefik dynamic dir, the `frps` config dir, and the configured Unix user's `~/.ssh` (authorized_keys write needs temp file + rename in that dir). Prefix optional paths with `-` so a missing dir doesn't fail the unit. Connecting to `/var/run/docker.sock` and talking to systemd need no write access (ASSUMPTION: unix-socket `connect` works on a read-only mount). If an operator later changes a configured path, `yanto install --reconfigure` re-renders the list; the dashboard shows a warning when a configured path is not in `ReadWritePaths` (ASSUMPTION). Fallback if this proves brittle: `ProtectSystem=full` (only `/usr`, `/boot`, `/etc` read-only — but `/etc/cron.d` and Traefik/frp dirs are in `/etc`, so they would still need `ReadWritePaths`). |
| `ProtectHome=read-only` | The user's home must stay writable at `~/.ssh` only (via `ReadWritePaths`). `ProtectHome=yes` would hide homes entirely and break Authorized Keys — **do not use**. |
| `DOCKER_CONFIG=<data_dir>/docker` and `HOME=/root` | So `docker login` for Registry Credentials writes inside the data dir rather than `/root/.docker` (which is read-only under `ProtectSystem=strict`). Git/ssh use `-o UserKnownHostsFile=<data_dir>/ssh/known_hosts` for the same reason (01 §7.2). |

**Do NOT set** (each breaks a core function):

| Option | Breaks |
|---|---|
| `ProcSubset=pid` / `ProtectProc=invisible` (or higher) | `/proc/meminfo`, `/proc/stat`, process listing used for host usage and port checks. |
| `PrivateUsers=yes` | chown to the SSH user, root-owned files appearing as `nobody`, systemctl authority. |
| `PrivateNetwork=yes` | Everything (dashboard, Docker socket TCP, Cloudflare API). |
| `PrivateDevices=yes` | Not known to break, but gives little and risks `docker` CLI/tty behavior; omitted deliberately (ASSUMPTION). |
| `MemoryDenyWriteExecute=yes` | Inherited by children (`docker`, `git`, `ssh`); fragile with runtimes that map W+X; little gain for a static Rust binary. |
| `ProtectHome=yes` | see above. |
| `RestrictNamespaces=yes` | Probably safe (Docker CLI only talks to the daemon), but unverified; leave off until tested, then enable if the suite passes. |
| `SystemCallFilter=` | Child tools' syscall surface (git, ssh, docker CLI, tar, pg_dump via exec) is too wide to allow-list reliably. Revisit only with `systemd-analyze` + strace evidence. |

**Optional hardening profile** (`yanto install --hardened`, ASSUMPTION; off by default): `CapabilityBoundingSet=CAP_CHOWN CAP_DAC_OVERRIDE CAP_FOWNER CAP_KILL CAP_SETUID CAP_SETGID` (+ `AmbientCapabilities=` same). Needs `DAC_OVERRIDE`/`FOWNER`/`CHOWN` to edit user-owned `authorized_keys` and set ownership; `KILL` for signalling process groups. Most likely to break an unforeseen path (e.g. a `docker compose` plugin step needing another capability), so it must pass the full test pass before being offered.

Trust model reminder (ADR 0007): root + Docker socket is already root-equivalent; sandboxing here limits accidental damage and blast radius of a bug in Yanto itself, not a malicious Owner. State this in the README.

`TimeoutStopSec=45` exceeds the Deployment drain timeout (default 30 s). On stop, `KillMode=control-group` kills Yanto's own children (a running `docker compose up`), but **not** containers (dockerd owns them), so Projects keep running while Yanto restarts.

## 6. Operating Yanto

| Task | How |
|---|---|
| Status / logs | `systemctl status yanto`, `journalctl -u yanto -f` (JSON logs by default) |
| Health | `GET /healthz` on the bind address |
| Config change | edit `/etc/yanto/config.toml` (or `yanto install --reconfigure`), `systemctl restart yanto` |
| Lost Owner password | `sudo yanto owner-reset` (revokes the Owner's sessions, prints a one-time reset URL valid 24 h — parity with legacy) |
| Bootstrap API Token for MCP/CI | dashboard, or `sudo yanto token create --name <n> --scope read\|write\|admin` (prints once; ASSUMPTION that the CLI path exists) |
| Backups | dashboard, or Cron Entries calling `yanto backup …` (D14) |
| Diagnose host | `sudo yanto doctor` — same prereq + Host Daemon checks as the wizard (ASSUMPTION: new command) |
| DB migrations | automatic at `serve`; `yanto db migrate` runs them without serving |

## 7. `yanto self-update`

Flags: `--check` (report only), `--version <vX.Y.Z>` (default latest non-prerelease), `--allow-prerelease`, `--yes`, `--no-restart`, `--rollback`.

1. Determine own target triple (compile-time constant) and install path (`std::env::current_exe`); refuse if the binary is not `/usr/local/bin/yanto` unless `--force` (ASSUMPTION: avoid clobbering a dev build).
2. Query Releases: `GET https://api.github.com/repos/kodin00/yanto/releases/latest` (or `/tags/<v>`); unauthenticated GitHub API rate limit applies (60/h — from general knowledge, **unverified**); honor optional `GITHUB_TOKEN`. Compare semver; stop if not newer.
3. Download `yanto-<target>` + `SHA256SUMS` to a temp dir; **verify SHA256** exactly as `install.sh`. Verify the new binary runs: `<new> version` exits 0 and reports the expected version.
4. **Refuse downgrades** across migrations: the new binary embeds its migration list; if the DB records migrations the target binary does not know (sqlx reports a missing migration), abort before replacing anything.
5. If a Deployment is running: list it and wait (`--yes` proceeds; the shutdown path will mark it failed per 01 §6.1). ASSUMPTION.
6. **Back up** `yanto.db` (SQLite online backup into `backups/yanto/pre-update-<ts>.db`).
7. Atomic replace: write `/usr/local/bin/yanto.new` (0755), keep current as `/usr/local/bin/yanto.prev`, `rename` over `yanto`. (Linux permits replacing a running executable by rename.)
8. `systemctl restart yanto` (skipped with `--no-restart`), poll `/healthz` 30 s. On failure: restore `yanto.prev`, restart, report, exit non-zero. (The DB backup stays; forward-only migrations mean a rollback after a *successful* migration needs `--rollback` plus restoring that backup — print this explicitly.)
9. `--rollback`: restore `yanto.prev`, optionally restore the named pre-update DB backup (asks first).

Implemented by hand with `reqwest` + `sha2` (small, no extra dependency; a `self_update`-style crate is **unverified** and not needed).

## 8. Uninstall

`sudo yanto uninstall [--purge] [--remove-managed] [--stop-projects] [--remove-registry] [--yes]`. Default is conservative: it removes Yanto itself and anything that would otherwise keep granting access or running on a schedule, and leaves workloads and data.

| Item | Default | Flag |
|---|---|---|
| `systemctl disable --now yanto`, remove unit, `daemon-reload` | removed | — |
| `/usr/local/bin/yanto` (+ `.prev`) | removed | — |
| `/etc/cron.d/yanto` (Cron Entries, including scheduled backups) | removed (asks) | `--remove-managed` removes without asking; `--keep-managed` keeps |
| Managed block in the Unix user's `authorized_keys` | removed (asks) — leaving it would keep SSH access granted by a tool that no longer exists | same flags |
| Yanto's Traefik file (`yanto.yml` in the dynamic dir) | removed (asks) → those Hostnames stop routing | same flags |
| Project containers / Compose stacks | **left running** | `--stop-projects` runs `docker compose down` per Project (no volume deletion) |
| Hosted Registry container (`registry:2`) | left running | `--remove-registry` stops and removes the container (data kept unless `--purge`) |
| `/etc/yanto/config.toml`, data dir (DB, backups, Deploy Keys, `app.key`, registry data, Project checkouts) | **kept** | `--purge` deletes them after typing the hostname/`yes` confirmation (not with `--yes` alone — requires `--purge --yes`) |
| Cloudflare tunnels/DNS, `frps`, `cloudflared`, Traefik daemons | untouched (remote/operator-owned) | — |

Manual steps are also in the README for the case the binary is already gone: stop/disable the unit, delete the unit and binary, delete `/etc/cron.d/yanto`, delete the `# BEGIN/END YANTO MANAGED` block, delete `/etc/yanto` and `/var/lib/yanto` if desired.

## 9. Release pipeline (GitHub Actions)

**No GHCR/Docker image is published** (D6, ADR 0007). The TS-era image workflow, `Dockerfile`, `compose*.yml`, `Makefile`, and `scripts/docker-entrypoint.sh` are deleted when the Rust tree replaces `master` (D1).

Workflows (file names ASSUMPTION):

### 9.1 `ci.yml` — on PR and push to the default branch

| Job | Command | Notes |
|---|---|---|
| `fmt` | `cargo fmt --all --check` | |
| `clippy` | `cargo clippy --workspace --all-targets --locked -- -D warnings` | |
| `test` | `cargo test --workspace --locked` | unit + `sqlx::test` SQLite + host-file integration; no Docker |
| `sqlx-prepare` | `sqlx database setup …` then `cargo sqlx prepare --workspace --check` | fails if `.sqlx/` metadata is stale; **ASSUMPTION** on exact flags |
| `audit` | `cargo audit` | scheduled daily too (new advisories without code changes) |
| `gitleaks` | gitleaks over full history (`actions/checkout` with `fetch-depth: 0`) | secrets management; blocks merges |
| `docker-tests` | `YANTO_DOCKER_TESTS=1 cargo test --workspace --locked -- --ignored` on `ubuntu-latest` (Docker preinstalled — ASSUMPTION) | gated: required on `main` and release tags, optional label-triggered on PRs (cost) |
| `musl-build` | `cargo build --release --locked --target <musl>` on both arches | catches static-link breakage before tagging (builds run on native runners: `ubuntu-latest` for x86_64, an arm64 hosted runner for aarch64 — label e.g. `ubuntu-24.04-arm` is **unverified**; fallback: `cargo-zigbuild`, also unverified) |

Tooling in CI comes from mise: `jdx/mise-action` with `install: true` and `cache: true` (docs show `@v4`/`@v5` in different snippets — pin to the latest major actually published, **unverified**), then `mise run <task>`; this keeps local and CI commands identical. Third-party actions pinned by commit SHA (supply-chain control, ASSUMPTION). `permissions: contents: read` by default.

### 9.2 `release.yml` — on tag `v*.*.*`

1. **verify**: tag matches `workspace.package.version`; CI (`ci.yml`) green on that commit; tags with a hyphen (`-rc.1`) are published as prerelease (so `releases/latest` ignores them).
2. **build** (matrix: `x86_64-unknown-linux-musl`, `aarch64-unknown-linux-musl`; native runner per arch): install musl toolchain (`musl-tools`), `mise install`, `cargo build --release --locked --target $T -p yanto-cli`, assert static (`file`/`ldd` shows "statically linked"/"not a dynamic executable"), smoke-run `yanto version` (on its own arch), upload as `yanto-$T`.
3. **checksums**: download both, `sha256sum yanto-* > SHA256SUMS`.
4. **publish** (`permissions: contents: write`, this job only): `gh release create "$TAG" --generate-notes [--prerelease] yanto-x86_64-unknown-linux-musl yanto-aarch64-unknown-linux-musl SHA256SUMS scripts/install.sh`.
5. Optional: build provenance attestation for each asset (`actions/attest-build-provenance`, **unverified** name/version) — supports open question 2.

Asset naming is fixed (no version in the file name) so `releases/latest/download/<name>` works from `install.sh` and `self-update`.

## 10. Full CLI command list

Binary `yanto` (`yanto-cli`, clap derive). Global: `--config <path>` (`YANTO_CONFIG`), `--log-level`, `--log-format json|pretty`, `-h/--help`, `-V` (alias for `version`). Clap features documented: `#[command(subcommand)]`, `#[arg(global = true)]`, `Arg::env` (needs the `env` feature).

| Command | Purpose | Needs root | Needs running `serve` |
|---|---|---|---|
| `yanto serve` | Run the dashboard, REST, MCP over HTTP, queue worker, retention tasks | yes | n/a |
| `yanto install` | Interactive/non-interactive installer (§3) | yes | no |
| `yanto uninstall` | Remove Yanto (§8) | yes | no |
| `yanto self-update` | Update the binary from GitHub Releases (§7) | yes | no (restarts it) |
| `yanto owner-reset` | Revoke the Owner's sessions and print a one-time reset URL (24 h) | yes (DB) | no |
| `yanto setup-code` | Issue a new one-time setup code while no Owner exists (ASSUMPTION: new) | yes (DB) | no |
| `yanto backup project <slug> [--volumes\|--db]` | Back up a Project's volumes and/or declared DB (D14) | yes | no |
| `yanto backup yanto` | SQLite online backup of Yanto's own DB | yes | no |
| `yanto backup list [--project <slug>]` | List Backups | yes | no |
| `yanto backup prune --keep <n> [--project <slug>]` | Delete older Backups | yes | no |
| `yanto mcp` | MCP over stdio; services run in-process; auth via `YANTO_API_TOKEN` (01 §4.1) | yes (DB) | no (Deployments wait queued if `serve` is down) |
| `yanto token create\|list\|revoke` | Manage API Tokens from the shell (ASSUMPTION: new) | yes (DB) | no |
| `yanto doctor` | Run prereq + Host Daemon + path checks (ASSUMPTION: new) | yes | no |
| `yanto db migrate` / `yanto db status` | Apply or show migrations without serving (ASSUMPTION) | yes | no |
| `yanto completions <shell>` | Print shell completions (`clap_complete`, **unverified**) | no | no |
| `yanto version` | Print version, git commit, target triple, build date | no | no |

Exit codes (ASSUMPTION): `0` success, `1` runtime error, `2` usage error (clap default), `3` prerequisite check failed (`install`, `doctor`).

`yanto backup …` is designed for use inside Cron Entries (`/etc/cron.d/yanto`): non-interactive, single-line output, non-zero exit on failure, takes a file lock so overlapping runs exit `0` with a skip notice (ASSUMPTION).

## 11. Open questions and risks

1. **Prerequisite installation**: legacy `install.sh` installed Docker via apt; D19 makes the script thin and D15 argues against Yanto installing daemons. Plan: print commands only. Confirm, or add an opt-in `--install-prereqs` for Debian/Ubuntu.
2. **Release authenticity**: SHA256 from the same Release covers corruption only. Consider signing (minisign/cosign) or provenance attestation, with the public key/identity embedded in `install.sh` and the binary for `self-update`. Needs a decision before v1.0.0.
3. **`curl | bash` + wizard**: relies on `/dev/tty`; behavior over non-TTY SSH sessions and cloud-init must be tested (`--non-interactive` path).
4. **systemd hardening is unverified**: Tier B especially (`ProtectSystem=strict` + generated `ReadWritePaths`). Must be exercised on Debian 12 and Ubuntu 22.04 with real Docker, cron, Traefik, and an `authorized_keys` write before shipping; record `systemd-analyze security` score.
5. **Operator-changed paths** (Traefik dir, frps config, SSH user) invalidate `ReadWritePaths` silently; needs a startup self-check that surfaces a clear error instead of EROFS.
6. **arm64 CI runners / musl + bundled SQLite** build details are unverified; fallback is `cargo-zigbuild`.
7. **Self-update rollback after migrations** is forward-only; DB restore is manual-ish. Acceptable for v1.0.0?
8. **Setup code lifetime** (24 h) and length are my assumptions; D18 only says "one-time".
9. **mise `cargo:` backend** and every tool pin are unverified; `mise.lock` commit policy to decide.
10. **GitHub API rate limit** for `self-update` on shared NAT IPs; token optional, `releases/latest/download` redirect path is the no-API fallback (needs the version for the checksums URL — resolve via the redirect `Location`).
