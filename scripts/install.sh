#!/usr/bin/env bash
set -euo pipefail
umask 077

REPO_URL="${YANTO_REPO_URL:-https://github.com/kodin00/yanto.git}"
BRANCH="${YANTO_BRANCH:-master}"
INSTALL_DIR="${YANTO_INSTALL_DIR:-/opt/yanto}"
PROJECTS_DIR="/var/lib/yanto/projects"
HOST_USER=""
PROJECTS_DIR_EXPLICIT=false
HOST_USER_EXPLICIT=false
ROLE="${1:-}"

usage() {
  echo "Usage: install.sh master|worker [--master URL] [--join-token TOKEN] [--name NAME] [--dir PATH] [--projects-dir PATH] [--host-user USER]"
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

random_secret() {
  openssl rand -hex 32 2>/dev/null | tr -d '\n' || date +%s%N
}

set_env_value() {
  local file="$1"
  local key="$2"
  local value="$3"
  local escaped
  escaped="$(printf '%s' "$value" | sed 's/[&|\\]/\\&/g')"
  if grep -q "^${key}=" "$file"; then
    sed -i "s|^${key}=.*|${key}=${escaped}|" "$file"
  else
    printf '%s=%s\n' "$key" "$value" >> "$file"
  fi
}

prepare_fresh_host_paths() {
  case "$PROJECTS_DIR" in
    /)
      echo "--projects-dir cannot be the filesystem root."
      exit 1
      ;;
    /*) ;;
    *)
      echo "--projects-dir must be an absolute path; '~' and relative paths are not supported."
      exit 1
      ;;
  esac

  local selected_user="$HOST_USER"
  if [ -z "$selected_user" ]; then
    if [ -n "${SUDO_USER:-}" ] && [ "$SUDO_USER" != "root" ]; then
      selected_user="$SUDO_USER"
    else
      selected_user="root"
    fi
  fi

  local passwd_entry
  passwd_entry="$(getent passwd "$selected_user" || true)"
  if [ -z "$passwd_entry" ]; then
    echo "Host user '$selected_user' does not exist."
    exit 1
  fi

  local host_home
  host_home="$(printf '%s' "$passwd_entry" | cut -d: -f6)"
  if [ -z "$host_home" ] || [ "${host_home#/}" = "$host_home" ]; then
    echo "Host user '$selected_user' does not have a valid absolute home directory."
    exit 1
  fi

  if [ -e "$PROJECTS_DIR" ] && [ ! -d "$PROJECTS_DIR" ]; then
    echo "Projects path '$PROJECTS_DIR' exists but is not a directory."
    exit 1
  fi
  if [ ! -d "$PROJECTS_DIR" ]; then
    install -d -m 0750 "$PROJECTS_DIR"
  fi
  SSH_SOURCE_DIR="$host_home/.ssh"
  if [ ! -d "$SSH_SOURCE_DIR" ]; then
    SSH_SOURCE_DIR="/var/lib/yanto/ssh-source"
    install -d -m 0700 "$SSH_SOURCE_DIR"
  fi
}

reject_path_overrides_for_existing_install() {
  local env_file="$1"
  if $PROJECTS_DIR_EXPLICIT || $HOST_USER_EXPLICIT; then
    echo "$env_file already exists; path overrides are only applied to fresh installs. Update that file deliberately if you want to move existing data."
    exit 1
  fi
}

write_master_env() {
  cd "$INSTALL_DIR"
  if [ ! -f .env ]; then
    prepare_fresh_host_paths
    cp .env.example .env
    set_env_value .env HOST_PROJECTS_ROOT "$PROJECTS_DIR"
    set_env_value .env HOST_AGENT_WORKTREES_ROOT "$PROJECTS_DIR/.yanto-worktrees"
    set_env_value .env SSH_SOURCE_DIR "$SSH_SOURCE_DIR"
  else
    reject_path_overrides_for_existing_install .env
  fi
  chmod 600 .env
  grep -q '^YANTO_NODE_ROLE=' .env && sed -i 's/^YANTO_NODE_ROLE=.*/YANTO_NODE_ROLE=master/' .env || echo 'YANTO_NODE_ROLE=master' >> .env
  grep -q '^JWT_SECRET=change-this-to-a-long-random-secret' .env && sed -i "s/^JWT_SECRET=.*/JWT_SECRET=$(random_secret)/" .env || true
  if grep -q '^YANTO_SETUP_CODE=' .env; then
    if [ -z "$(sed -n 's/^YANTO_SETUP_CODE=//p' .env | head -n 1)" ]; then
      sed -i "s/^YANTO_SETUP_CODE=.*/YANTO_SETUP_CODE=$(random_secret)/" .env
    fi
  else
    echo "YANTO_SETUP_CODE=$(random_secret)" >> .env
  fi
  grep -q '^POSTGRES_PASSWORD=' .env || echo "POSTGRES_PASSWORD=$(random_secret)" >> .env
  grep -q '^WORKER_JOIN_TOKEN=change-this-worker-join-token' .env && sed -i "s/^WORKER_JOIN_TOKEN=.*/WORKER_JOIN_TOKEN=$(random_secret)/" .env || true
  grep -q '^WORKER_TOKEN_SECRET=change-this-worker-token-secret' .env && sed -i "s/^WORKER_TOKEN_SECRET=.*/WORKER_TOKEN_SECRET=$(random_secret)/" .env || true
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
  if [ -f .env.worker ]; then
    reject_path_overrides_for_existing_install .env.worker
    set_env_value .env.worker YANTO_MASTER_URL "$master_url"
    set_env_value .env.worker WORKER_JOIN_TOKEN "$join_token"
    set_env_value .env.worker YANTO_WORKER_NAME "$worker_name"
    chmod 600 .env.worker
    return
  fi
  prepare_fresh_host_paths
  cat > .env.worker <<EOF
YANTO_NODE_ROLE=worker
YANTO_MASTER_URL=$master_url
WORKER_JOIN_TOKEN=$join_token
YANTO_WORKER_TOKEN=
YANTO_WORKER_NAME=$worker_name
HOST_PROJECTS_ROOT=$PROJECTS_DIR
SSH_SOURCE_DIR=$SSH_SOURCE_DIR
COMMAND_TIMEOUT_MS=3600000
COMMAND_OUTPUT_MAX_BYTES=2097152
EOF
  chmod 600 .env.worker
}

run_master() {
  cd "$INSTALL_DIR"
  docker compose -f compose.yml --env-file .env up -d --build
  echo "Yanto is running."
  if grep -q '^ADMIN_USERNAME=.' .env && grep -q '^ADMIN_PASSWORD=.' .env; then
    echo "The legacy admin credentials in $INSTALL_DIR/.env will be migrated to the database owner."
  else
    echo "Open Yanto and create the first owner with this one-time setup code:"
    sed -n 's/^YANTO_SETUP_CODE=//p' .env | head -n 1
  fi
}

run_worker() {
  cd "$INSTALL_DIR"
  docker compose -f compose.worker.yml --env-file .env.worker up -d --build
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
    --projects-dir)
      PROJECTS_DIR="${2:-}"
      PROJECTS_DIR_EXPLICIT=true
      shift 2
      ;;
    --host-user)
      HOST_USER="${2:-}"
      HOST_USER_EXPLICIT=true
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
    write_master_env
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
