#!/usr/bin/env bash
# Yanto installer: fetch the release binary, verify it, hand off to `yanto install`.
#
#   curl -fsSL https://github.com/kodin00/yanto/releases/latest/download/install.sh | sudo bash
#   curl -fsSL .../install.sh | sudo bash -s -- --non-interactive --ssh-user deploy
#
# Environment:
#   YANTO_VERSION  release tag to install (default: latest non-prerelease)
#   YANTO_REPO     owner/repo override (forks, testing); default kodin00/yanto
#
# This script only writes /usr/local/bin/yanto. It never edits other system
# files, never starts a service and never touches Docker; all of that is
# `yanto install`. Every argument is passed through to the wizard.
set -euo pipefail
umask 022

REPO="${YANTO_REPO:-kodin00/yanto}"
VERSION="${YANTO_VERSION:-}"
DEST="/usr/local/bin/yanto"

say() { printf '%s\n' "$*"; }
die() { printf 'install.sh: error: %s\n' "$*" >&2; exit 1; }

# 1. Preconditions -----------------------------------------------------------
[ "$(uname -s)" = "Linux" ] || die "Linux only (found $(uname -s))"

if [ "$(id -u)" -ne 0 ]; then
  # Re-exec through sudo only when this is a real file; under `curl | bash`
  # the script body is already consumed, so ask for `| sudo bash` instead.
  if command -v sudo >/dev/null 2>&1 && [ -f "${BASH_SOURCE[0]:-}" ]; then
    exec sudo -E bash "${BASH_SOURCE[0]}" "$@"
  fi
  die "must run as root: curl -fsSL https://github.com/${REPO}/releases/latest/download/install.sh | sudo bash"
fi

if command -v curl >/dev/null 2>&1; then
  fetch() { curl -fsSL --proto '=https' --tlsv1.2 --retry 3 -o "$2" "$1"; }
elif command -v wget >/dev/null 2>&1; then
  fetch() { wget -q --https-only --tries=3 -O "$2" "$1"; }
else
  die "curl or wget is required (this script does not install it)"
fi
for tool in sha256sum uname install mktemp; do
  command -v "$tool" >/dev/null 2>&1 || die "required tool not found: $tool (this script does not install it)"
done

# 2. Architecture ------------------------------------------------------------
case "$(uname -m)" in
  x86_64 | amd64) TARGET="x86_64-unknown-linux-musl" ;;
  aarch64 | arm64) TARGET="aarch64-unknown-linux-musl" ;;
  *) die "unsupported architecture $(uname -m); supported: x86_64, aarch64" ;;
esac
ASSET="yanto-${TARGET}"

# 3. Resolve URLs ------------------------------------------------------------
if [ -n "$VERSION" ]; then
  BASE="https://github.com/${REPO}/releases/download/${VERSION}"
else
  BASE="https://github.com/${REPO}/releases/latest/download"
fi

# 4. Download into a private temp dir -----------------------------------------
TMP="$(mktemp -d)"
chmod 0700 "$TMP"
cleanup() { rm -rf "$TMP"; }
trap cleanup EXIT

say "Downloading ${ASSET} (${VERSION:-latest}) from ${REPO}..."
fetch "${BASE}/${ASSET}" "${TMP}/${ASSET}" || die "download failed: ${BASE}/${ASSET}"
fetch "${BASE}/SHA256SUMS" "${TMP}/SHA256SUMS" || die "download failed: ${BASE}/SHA256SUMS"

# 5. Verify ------------------------------------------------------------------
# (a) integrity: checksum from the same release (catches corruption/truncation).
expected="$(awk -v n="$ASSET" '{ f=$2; sub(/^\*/, "", f); if (f == n) { print $1; exit } }' "${TMP}/SHA256SUMS")"
[ -n "$expected" ] || die "SHA256SUMS has no entry for ${ASSET}"
printf '%s  %s\n' "$expected" "$ASSET" | (cd "$TMP" && sha256sum -c --status -) \
  || die "SHA256 mismatch for ${ASSET}; aborting"
say "SHA256 verified."

# (b) authenticity: build provenance attestation, when the GitHub CLI exists.
if command -v gh >/dev/null 2>&1; then
  gh attestation verify "${TMP}/${ASSET}" --repo "$REPO" >/dev/null \
    || die "gh attestation verify failed for ${ASSET}; aborting"
  say "Build provenance attestation verified."
else
  say "Note: gh not found; skipping attestation check (SHA256 proves integrity only)."
fi

# 6. Atomic install ----------------------------------------------------------
mkdir -p "$(dirname "$DEST")"
install -m 0755 -o root -g root "${TMP}/${ASSET}" "${DEST}.new"
mv -f "${DEST}.new" "$DEST"
say "Installed $("$DEST" version)."

# 7. Hand off to the wizard ----------------------------------------------------
# Under `curl | bash` stdin is the script pipe; give the wizard the terminal.
# A fresh bash redirect probe is used because /dev/tty can exist yet be unusable.
cleanup
trap - EXIT
if [ ! -t 0 ] && ( : </dev/tty ) 2>/dev/null; then
  exec "$DEST" install "$@" </dev/tty
fi
exec "$DEST" install "$@"
