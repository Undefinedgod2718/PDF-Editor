#!/bin/bash
set -euo pipefail
# Open TCP 8050. Needs passwordless sudo on host, or run ufw manually.
ss -lnt | grep 8050 || true
if command -v ufw >/dev/null 2>&1; then
  if sudo -n true 2>/dev/null; then
    sudo -n ufw allow 8050/tcp comment 'PDF Editor' || true
    sudo -n ufw status numbered | grep -E '8050|Status' || true
  else
    echo "NOTE: passwordless sudo unavailable — ensure ufw already allows 8050/tcp" >&2
  fi
fi
curl -s -o /dev/null -w "local=%{http_code}\n" http://127.0.0.1:8050/api/documents
curl -s -o /dev/null -w "lan=%{http_code}\n" http://192.168.17.56:8050/api/documents || true
echo FIREWALL_DONE
