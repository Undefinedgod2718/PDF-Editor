#!/bin/bash
# Hotfix body-limit (or any server-only sync). Requires SSHPASS — no hardcoded secrets.
set -euo pipefail
export PATH="/mnt/c/tools/bin:$HOME/.cargo/bin:$PATH"
HOST="${HOST:-richard@192.168.17.56}"
ROOT="${ROOT:-/mnt/d/Program_Coding/PDF Editor}"

if [ -f "$ROOT/deploy/.env" ]; then
  set -a
  eval "$(tr -d '\015' < "$ROOT/deploy/.env" | grep -E '^[A-Za-z_][A-Za-z0-9_]*=' || true)"
  set +a
fi
if [ -z "${SSHPASS:-}" ]; then
  echo "FATAL: SSHPASS unset (or deploy/.env)." >&2
  exit 1
fi
export SSHPASS

BIN=/mnt/d/DockerRoot/pdf-editor/bin
tar -C "$ROOT" --exclude='server/target' --exclude='server/pdfium.dll' -czf /tmp/pdf-server-src.tgz server
sshpass -e scp -o StrictHostKeyChecking=no /tmp/pdf-server-src.tgz "$HOST:/tmp/pdf-server-src.tgz"
sshpass -e ssh -o StrictHostKeyChecking=no "$HOST" bash -s <<'REMOTE'
set -euo pipefail
export PATH="/mnt/c/tools/bin:$HOME/.cargo/bin:$PATH"
APP=/mnt/d/DockerRoot/pdf-editor
SRC=$APP/src
BIN=$APP/bin
mkdir -p "$SRC"
tar -xzf /tmp/pdf-server-src.tgz -C "$SRC"
cd "$SRC/server"
unset CARGO_TARGET_DIR || true
export CARGO_TARGET_DIR="$SRC/server/target"
cargo build --release
cp -f "$SRC/server/target/release/pdf-editor-server" "$BIN/pdf-editor-server"
systemctl --user restart pdf-editor.service
sleep 2
systemctl --user --no-pager status pdf-editor.service | head -15
curl -s -o /dev/null -w "local=%{http_code}\n" http://127.0.0.1:8050/api/documents
echo HOTFIX_OK
REMOTE
