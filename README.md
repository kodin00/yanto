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

## Docker Runtime

The app is designed to run with:

- `/var/run/docker.sock:/var/run/docker.sock` so it can inspect and control Docker.
- `${HOST_PROJECTS_ROOT:-~/projects}:/projects` so registered projects are cloned or deployed from the host projects folder.
- `${SSH_SOURCE_DIR:-~/.ssh}:/root/.ssh:ro` so Git deployments use the SSH key you configured on the VPS.
- A persistent SSH volume at `/data/ssh` is reserved for app-managed keys.

Docker socket access is powerful. Run this only for your own trusted admin dashboard.

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
- `COMMAND_TIMEOUT_MS`, default `3600000` (one hour) for Git, Docker, and backup helper commands
- `COMMAND_OUTPUT_MAX_BYTES`, default `2097152`, caps in-memory command output while still streaming deployment logs
- `DEPLOYMENT_LOG_MAX_CHARS`, default `500000`, keeps recent deployment logs bounded in Postgres

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
