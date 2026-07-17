#!/bin/bash
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
tr -d '\015' < "$ROOT/deploy/on56_open_8050.sh" > /tmp/on56_open_8050.sh
sshpass -e scp -o StrictHostKeyChecking=no /tmp/on56_open_8050.sh richard@192.168.17.56:/tmp/on56_open_8050.sh
sshpass -e ssh -o StrictHostKeyChecking=no richard@192.168.17.56 \
  "tr -d '\015' < /tmp/on56_open_8050.sh > /tmp/o8050.sh && bash /tmp/o8050.sh"
