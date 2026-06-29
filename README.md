# Yanto Deploy

A small personal deployment platform for a single VPS. It registers Git projects, deploys them with Docker Compose, exposes a token-protected deploy endpoint, and gives you a compact dashboard for deployments, containers, logs, and host usage.

## Quick Start

```bash
cp .env.example .env
npm install
npm run build
make docker
```

Open `http://localhost:8080` and sign in with `ADMIN_USERNAME` and `ADMIN_PASSWORD` from `.env`.

## One-Line Install

Master node:

```bash
curl -fsSL https://raw.githubusercontent.com/kodin00/yanto/master/scripts/install.sh | sudo bash -s -- master
```

Worker node:

```bash
curl -fsSL https://raw.githubusercontent.com/kodin00/yanto/master/scripts/install.sh | sudo bash -s -- worker --master http://MASTER_IP:PORT --join-token TOKEN
```

The installer supports Ubuntu/Debian hosts. It installs Git and Docker when missing, clones Yanto into `/opt/yanto`, and starts Docker Compose.

## Docker Runtime

The app is designed to run with:

- `/var/run/docker.sock:/var/run/docker.sock` so it can inspect and control Docker.
- `${HOST_PROJECTS_ROOT:-~/projects}:/projects` so registered projects are cloned or deployed from the host projects folder.
- `${SSH_SOURCE_DIR:-~/.ssh}:/root/.ssh:ro` so Git deployments use the SSH key you configured on the VPS.
- A persistent SSH volume at `/data/ssh` is reserved for app-managed keys.

Docker socket access is powerful. Run this only for your own trusted admin dashboard.

## Multi-Node Runtime

Yanto has two runtime roles:

- `YANTO_NODE_ROLE=master` runs the dashboard, API, database migrations, and local deploy worker.
- `YANTO_NODE_ROLE=worker` runs only the headless worker process.

Workers poll the master over outbound HTTP. They do not serve the web UI and `compose.worker.yml` does not publish ports.

Set these on the master before connecting workers if you want to provide your own stable secrets:

```env
WORKER_JOIN_TOKEN=<long-random-token>
WORKER_TOKEN_SECRET=<long-random-secret>
APP_BASE_URL=http://MASTER_IP:PORT
```

Worker installs write `.env.worker` with `YANTO_MASTER_URL`, `WORKER_JOIN_TOKEN`, and an optional `YANTO_WORKER_NAME`. After registration, the worker stores its persistent token in the `yanto_worker_data` Docker volume.

If `WORKER_JOIN_TOKEN` is not set, Yanto generates and stores one the first time you copy the worker install command from Settings.

## Deploy Webhook

Each project shows:

```bash
curl -X POST "$APP_BASE_URL/deploy?id=<project-id>" \
  -H "Authorization: Bearer <project-deploy-token>"
```

The webhook and manual deploy button both run the same flow:

1. Clone the repo into `/projects/<folder>` if the folder does not exist.
2. Leave an existing folder untouched.
3. Fetch, checkout, and fast-forward pull the configured branch when the folder is a Git repo.
4. Run `docker compose -f <compose-file> up -d --build`.
5. Store status and build logs.

## Configuration

Important environment variables:

- `DATABASE_URL`
- `JWT_SECRET`
- `ADMIN_USERNAME`
- `ADMIN_PASSWORD`
- `PROJECTS_ROOT` inside the container, default `/projects`
- `HOST_PROJECTS_ROOT` for display, default `~/projects`
- `SSH_SOURCE_DIR` host SSH folder mounted read-only into the app, for example `/home/ubuntu/.ssh`
- `SSH_PRIVATE_KEY_PATH` private key path inside the app container, default `/root/.ssh/id_ed25519`
- `SSH_KEYS_DIR`, default `/data/ssh`
- `APP_BASE_URL`
- `YANTO_NODE_ROLE`, default `master`
- `WORKER_JOIN_TOKEN` for worker registration
- `WORKER_TOKEN_SECRET` for worker token signing/hashing configuration
- `YANTO_MASTER_URL` for worker nodes
- `YANTO_WORKER_TOKEN` optional pre-provisioned worker token
- `YANTO_WORKER_NAME` optional display name for worker nodes
- `COMMAND_TIMEOUT_MS`, default `3600000` (one hour) for Git, Docker, and backup helper commands
- `COMMAND_OUTPUT_MAX_BYTES`, default `2097152`, caps in-memory command output while still streaming deployment logs
- `DEPLOYMENT_LOG_MAX_CHARS`, default `500000`, keeps recent deployment logs bounded in Postgres
- `FRP_BIND_PORT`, default `7000`, is the public FRPC control port
- `FRP_PORT_START` and `FRP_PORT_END`, defaults `25560` and `25600`, define the published TCP/UDP forwarding range

## FRP Port Forwarding

The FRP screen manages TCP and UDP services on enrolled worker nodes, including home servers behind CGNAT. Set the VPS public IP or an unproxied DNS hostname in the FRP screen, copy the worker install command to the home server, and create a forwarding rule.

See [FRP_GUIDE.md](FRP_GUIDE.md) for setup, daily operations, and troubleshooting.

The VPS firewall must allow the FRP control port and configured forwarding range. With the defaults:

```bash
sudo ufw allow 7000/tcp
sudo ufw allow 25560:25600/tcp
sudo ufw allow 25560:25600/udp
```

Yanto does not change firewall rules automatically. A Cloudflare-proxied hostname cannot carry arbitrary Minecraft TCP/UDP traffic; use the VPS IP or a DNS-only hostname.

The worker reaches the default local target through `host.docker.internal`. A native Minecraft server or Docker-published Minecraft port should therefore listen on a host interface reachable from Docker, such as `0.0.0.0:25565`; otherwise enter a reachable LAN address in the tunnel form.

Existing workers must be rebuilt after upgrading Yanto so their image contains the pinned FRPC binary. Re-running the worker install command performs the repository update and Compose rebuild.

## Git SSH On A VPS

If Git deploys fail with `Permission denied (publickey)`, test from inside the app container, not only on the host:

```bash
docker compose -f compose.yml exec app ssh -T git@github.com
docker compose -f compose.yml exec app ls -la /root/.ssh
```

Your `.env` should point at the host user's SSH folder:

```env
SSH_SOURCE_DIR=/home/ubuntu/.ssh
SSH_PRIVATE_KEY_PATH=/root/.ssh/id_ed25519
```

Then restart:

```bash
docker compose -f compose.yml up -d --build
```

Alternatively, paste a private key in `Settings -> Git SSH key`. Yanto stores it in `/data/ssh/id_ed25519`, which is persisted by the `yanto_ssh` Docker volume, and uses that key before any mounted VPS key.

## Development

```bash
npm run dev
npm run dev:client
npm run db:generate
npm run db:push
npm run typecheck
npm test
npm run lint
```

## GitHub Actions Deploy

Pushes to `master` run typecheck, lint, tests, build, then deploy to the VPS over SSH.

Add these repository secrets in GitHub under `Settings -> Secrets and variables -> Actions`:

- `VPS_HOST`: VPS hostname or IP address
- `VPS_USER`: SSH user, for example `ubuntu`
- `VPS_SSH_KEY`: private SSH key allowed to log in to the VPS
- `VPS_APP_DIR`: absolute repo path on the VPS, for example `/home/ubuntu/yanto`
- `VPS_PORT`: optional SSH port, defaults to `22`

The SSH user must be able to run Docker with passwordless sudo.

The deploy command is:

```bash
git pull --ff-only origin master
sudo -n docker compose -f compose.yml up -d --build --remove-orphans
```
