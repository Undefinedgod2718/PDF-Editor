#!/bin/bash
# Run the sgsac001 log watch from the dev box via WSL:
#   wsl bash "/mnt/d/Program_Coding/PDF Editor/deploy/_wsl_watch_logs.sh"
set -euo pipefail
ROOT="/mnt/d/Program_Coding/PDF Editor"
if [ -f "$ROOT/deploy/.env" ]; then
  set -a
  eval "$(tr -d '\015' < "$ROOT/deploy/.env" | grep -E '^[A-Za-z_][A-Za-z0-9_]*=' || true)"
  set +a
fi
if [ -z "${SSHPASS:-}" ]; then
  echo "FATAL: SSHPASS unset." >&2
  exit 1
fi
export SSHPASS
SRC="$ROOT/deploy/on56_watch_logs.sh"
tr -d '\015' < "$SRC" > /tmp/on56_watch_logs.sh
chmod +x /tmp/on56_watch_logs.sh
sshpass -e scp -o StrictHostKeyChecking=no /tmp/on56_watch_logs.sh richard@192.168.17.56:/tmp/on56_watch_logs.sh
sshpass -e ssh -o StrictHostKeyChecking=no richard@192.168.17.56 bash /tmp/on56_watch_logs.sh
