#!/usr/bin/env bash
set -euo pipefail
umask 077

REPO_URL="${YANTO_REPO_URL:-https://github.com/kodin00/yanto.git}"
BRANCH="${YANTO_BRANCH:-master}"
INSTALL_DIR="${YANTO_INSTALL_DIR:-/opt/yanto}"
ROLE="${1:-}"

usage() {
  echo "Usage: install.sh master|worker [--master URL] [--join-token TOKEN] [--name NAME] [--dir PATH]"
}

need_root() {
  if [ "$(id -u)" -ne 0 ]; then
    echo "Run with sudo/root."
    exit 1
  fi
}

install_packages() {
  if command -v git >/dev/null 2>&1 && command -v docker >/dev/null 2>&1; then
    return
  fi
  if ! command -v apt-get >/dev/null 2>&1; then
    echo "Automatic prerequisite install currently supports Ubuntu/Debian only."
    exit 1
  fi
  apt-get update
  apt-get install -y --no-install-recommends ca-certificates curl git docker.io docker-compose-plugin
  systemctl enable --now docker >/dev/null 2>&1 || true
}

clone_or_update() {
  if [ -d "$INSTALL_DIR/.git" ]; then
    git -C "$INSTALL_DIR" fetch origin "$BRANCH"
    git -C "$INSTALL_DIR" checkout "$BRANCH"
    git -C "$INSTALL_DIR" pull --ff-only origin "$BRANCH"
  else
    mkdir -p "$(dirname "$INSTALL_DIR")"
    git clone --branch "$BRANCH" "$REPO_URL" "$INSTALL_DIR"
  fi
}

write_worker_env() {
  local master_url="$1"
  local join_token="$2"
  local worker_name="$3"
  if [ -z "$master_url" ] || [ -z "$join_token" ]; then
    echo "Worker install requires --master URL and --join-token TOKEN."
    exit 1
  fi
  cd "$INSTALL_DIR"
  cat > .env.worker <<EOF
YANTO_NODE_ROLE=worker
YANTO_MASTER_URL=$master_url
WORKER_JOIN_TOKEN=$join_token
YANTO_WORKER_TOKEN=
YANTO_WORKER_NAME=$worker_name
HOST_PROJECTS_ROOT=/opt/yanto-projects
SSH_SOURCE_DIR=/root/.ssh
COMMAND_TIMEOUT_MS=3600000
COMMAND_OUTPUT_MAX_BYTES=2097152
EOF
  chmod 600 .env.worker
}

run_master() {
  cd "$INSTALL_DIR"
  docker compose -f compose.yml pull
  docker compose -f compose.yml up -d
  echo "Yanto is running. Open http://127.0.0.1:8080 and create the first owner."
}

run_worker() {
  cd "$INSTALL_DIR"
  docker compose -f compose.worker.yml --env-file .env.worker pull
  docker compose -f compose.worker.yml --env-file .env.worker up -d
}

need_root

if [ -z "$ROLE" ]; then
  usage
  exit 1
fi
shift || true

MASTER_URL=""
JOIN_TOKEN=""
WORKER_NAME="$(hostname)"
while [ "$#" -gt 0 ]; do
  case "$1" in
    --master)
      MASTER_URL="${2:-}"
      shift 2
      ;;
    --join-token)
      JOIN_TOKEN="${2:-}"
      shift 2
      ;;
    --name)
      WORKER_NAME="${2:-}"
      shift 2
      ;;
    --dir)
      INSTALL_DIR="${2:-}"
      shift 2
      ;;
    *)
      usage
      exit 1
      ;;
  esac
done

install_packages
clone_or_update

case "$ROLE" in
  master)
    run_master
    ;;
  worker)
    write_worker_env "$MASTER_URL" "$JOIN_TOKEN" "$WORKER_NAME"
    run_worker
    ;;
  *)
    usage
    exit 1
    ;;
esac
