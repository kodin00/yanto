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
- `SSH_SOURCE_DIR` host SSH folder mounted read-only into the app, default `~/.ssh`
- `SSH_KEYS_DIR`, default `/data/ssh`
- `APP_BASE_URL`

## Development

```bash
npm run dev
npm run dev:client
npm run typecheck
npm test
npm run lint
```
