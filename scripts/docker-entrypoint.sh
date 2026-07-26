#!/bin/sh
set -eu

secret_dir="${YANTO_CONFIG_DIR:-/data/config}"
mkdir -p "$secret_dir"
chmod 700 "$secret_dir"

random_secret() {
  od -An -N32 -tx1 /dev/urandom | tr -d ' \n'
}

if [ -z "${JWT_SECRET:-}" ]; then
  if [ ! -s "$secret_dir/jwt-secret" ]; then
    umask 077
    random_secret > "$secret_dir/jwt-secret"
  fi
  JWT_SECRET="$(tr -d '\r\n' < "$secret_dir/jwt-secret")"
  export JWT_SECRET
fi

if [ -z "${WORKER_TOKEN_SECRET:-}" ]; then
  if [ ! -s "$secret_dir/worker-token-secret" ]; then
    umask 077
    random_secret > "$secret_dir/worker-token-secret"
  fi
  WORKER_TOKEN_SECRET="$(tr -d '\r\n' < "$secret_dir/worker-token-secret")"
  export WORKER_TOKEN_SECRET
fi

exec "$@"
