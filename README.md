# Yanto

Yanto is a self-hosted deployment and operations tool for **one Linux server**. It deploys Projects (from a Git repository or a container image) with Docker Compose and operates their containers. It also manages the Host's cron entries, SSH authorized keys and Host Daemons, and can run a private container registry.

It is one static Rust binary that runs as a systemd service. It is built for DevOps tasks only: it has no AI features and never edits your application code.

## Install

Requirements: Linux with systemd (Debian 12+ or Ubuntu 22.04+, amd64 or arm64), Docker with the Compose plugin, and Git. The installer checks for them and prints what is missing; it does not install anything for you.

```sh
curl -fsSL https://github.com/kodin00/yanto/releases/latest/download/install.sh | sudo bash
```

The script downloads the release binary to `/usr/local/bin/yanto`, verifies its SHA256 (and its build attestation when the `gh` CLI is present), then runs `yanto install`. The wizard writes `/etc/yanto/config.toml` and the systemd unit, starts the service and prints a one-time **setup code**. Every prompt has a flag; for unattended installs use `--non-interactive`:

```sh
curl -fsSL https://github.com/kodin00/yanto/releases/latest/download/install.sh \
  | sudo bash -s -- --non-interactive --ssh-user deploy
```

### Build from source

[mise](https://mise.jdx.dev) pins the Rust toolchain and dev tools.

```sh
git clone https://github.com/kodin00/yanto && cd yanto
mise install
mise x -- cargo build --release -p yanto-cli
sudo ./target/release/yanto install
```

## First run

1. Open the dashboard. It listens on `127.0.0.1:9197` by default, so from your own machine use an SSH tunnel:

   ```sh
   ssh -L 9197:127.0.0.1:9197 you@your-server
   ```

   then browse to <http://127.0.0.1:9197>.
2. Enter the setup code and create the first Owner account. If you lost the code, run `sudo yanto setup-code` (works only while no Owner exists).
3. Create a Project, deploy it, and optionally put a Hostname in front of it.

To expose the dashboard on a real domain, set `public_url` and put it behind your own Traefik or Cloudflare Tunnel (see `yanto install --help`). Keep it off the open internet unless you need it.

## Features

- **Projects and Deployments**: Git or image Projects, compose and `.env` editing, queued Deployments with live logs, stop, restart and rollback to the exact previous config.
- **Webhooks**: `POST /deploy` with a Deploy Token and a GitHub push webhook.
- **Containers**: list, start/stop/restart, logs, and an exec terminal in the dashboard (needs the `runtime` permission, audited).
- **Users**: one Owner plus Members with per-Project permissions; invites, password resets, revocable sessions.
- **Hostnames**: route a Project's loopback port through Cloudflare Tunnel or host Traefik, with Cloudflare zone and DNS management.
- **Hosted Registry**: a `registry:2` container on `127.0.0.1:5000` with users, a repo/tag browser and garbage collection. Registry Credentials pull private external images.
- **Host management**: Cron Entries in `/etc/cron.d/yanto` with run history, Authorized Keys in a marked block, Deploy Keys for private Git repos.
- **Backups**: local and downloadable: Project volumes, Postgres/MySQL dumps and Yanto's own database. Schedule them with Cron Entries that call `yanto backup`.
- **System**: host usage, a protected Docker prune with preview, health, audit log and runtime settings.
- **API and MCP**: a REST API and an MCP server, both authenticated by API Tokens.

### Host Daemons

`cloudflared`, `frps` and Traefik are **Host Daemons**: you install and configure them on the server. Yanto only checks that they are present and running, shows install hints when they are not, and drives them (for example restarting `frps` after you confirm a config change). It never installs them. For `frps` it manages the server side only; `frpc` clients are yours to set up.

## Trust model

Read this before installing.

- Yanto runs as **root**, on purpose. Access to the Docker socket is already root-equivalent on the Host, so a non-root user would add moving parts without a real security boundary. Yanto writes `/etc/cron.d/yanto`, edits `authorized_keys` for the configured user and controls Host Daemons.
- **The Owner account and the Host are fully trusted.** The Owner can run containers and commands as root through Deployments, Cron Entries and Authorized Keys. Anyone who becomes the Owner owns the server.
- Members are limited to the Projects and permissions the Owner grants. A bug in permission checks is a host compromise, so only add Members you trust with a Project's code and containers.
- Yanto manages only the machine it runs on. Secrets at rest are encrypted with a key at `/var/lib/yanto/keys/app.key`; the Yanto database backup includes that key and is downloadable by the Owner only.

## API Tokens, REST and MCP

Create a token in the dashboard under **API tokens**. A token acts as its user, capped by a scope: `read` < `write` < `admin`. It is shown once. Tokens work only on `/api/v1` and `/mcp`, never on dashboard pages.

REST:

```sh
curl -H "Authorization: Bearer $YANTO_TOKEN" http://127.0.0.1:9197/api/v1/projects
curl -X POST -H "Authorization: Bearer $YANTO_TOKEN" http://127.0.0.1:9197/api/v1/projects/<id>/deploy
```

Errors look like `{"error":{"code","message"}}`. The endpoints and their required scopes are listed in [docs/rewrite/03-interfaces.md](docs/rewrite/03-interfaces.md).

MCP:

- Over HTTP, point your client at `http://127.0.0.1:9197/mcp` with the `Authorization: Bearer` header.
- Over stdio, run `yanto mcp` with the token in a file (mode 0600) named by `YANTO_API_TOKEN_FILE`. The token is never taken from command-line arguments.

Sensitive tools require `confirm: true`. Container exec is not available over REST or MCP.

## Upgrade and uninstall

```sh
sudo yanto self-update --check   # is there a newer release?
sudo yanto self-update           # verify, copy the database, swap the binary, restart
sudo yanto self-update --rollback
```

Rolling back after a database migration means restoring the pre-update copy under `/var/lib/yanto/backups/yanto/` by hand while the service is stopped.

```sh
sudo yanto uninstall             # keeps your data
sudo yanto uninstall --purge     # also deletes config, database, backups and keys
```

Other commands: `yanto doctor`, `yanto owner-reset`, `yanto backup`, `yanto version`.

## Development

Everything runs through [mise](https://mise.jdx.dev):

```sh
mise install        # Rust, cargo-audit, gitleaks, sqlx-cli
mise run dev        # serve against ./dev-data on 127.0.0.1:9197
mise run fmt        # cargo fmt --check
mise run clippy     # clippy with warnings denied
mise run test       # tests (no Docker needed)
mise run audit      # cargo audit
mise run secrets    # gitleaks
mise run ci         # all of the above
mise run build      # static musl release build (Linux with musl-tools)
```

Docker-backed tests are ignored by default: `cargo test -p yanto-host -- --ignored`.

The design lives in [docs/rewrite](docs/rewrite) and [docs/adr](docs/adr); vocabulary is in [CONTEXT.md](CONTEXT.md); the legacy TypeScript system is described in [docs/legacy](docs/legacy). See [CHANGELOG.md](CHANGELOG.md) for release notes.
