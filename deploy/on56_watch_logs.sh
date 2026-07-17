#!/bin/bash
# Log watch for PDF Editor on sgsac001. Runs ON the remote host.
# Incremental: remembers last-scanned byte offset in logs/.watch_offset and
# reports only NEW lines since the previous run. Exit codes:
#   0 = no new errors    1 = new ERROR lines found    2 = service/health down
# Cron example (every 5 min):
#   */5 * * * * /mnt/d/DockerRoot/pdf-editor/bin/on56_watch_logs.sh >> /mnt/d/DockerRoot/pdf-editor/logs/watch.log 2>&1
set -uo pipefail

APP="${APP:-/mnt/d/DockerRoot/pdf-editor}"
LOG="$APP/logs/service.log"
STATE="$APP/logs/.watch_offset"
FAIL=0

echo "=== $(date -Is) log watch ==="

# --- service alive? ---
if ! systemctl --user is-active --quiet pdf-editor.service 2>/dev/null; then
  echo "ALERT: pdf-editor.service not active"
  FAIL=2
fi

# --- health endpoint (detects broken sidecar even with zero traffic) ---
HEALTH=$(curl -s -m 5 http://127.0.0.1:8050/api/health || echo CURL_FAIL)
case "$HEALTH" in
  CURL_FAIL)
    echo "ALERT: /api/health unreachable"
    FAIL=2
    ;;
  *'"ok":false'*)
    echo "ALERT: sidecar unhealthy: $HEALTH"
    [ "$FAIL" -eq 0 ] && FAIL=1
    ;;
  *)
    echo "health: ok"
    ;;
esac

# --- incremental ERROR scan ---
if [ ! -f "$LOG" ]; then
  echo "ALERT: log file missing: $LOG"
  exit 2
fi

SIZE=$(stat -c%s "$LOG")
LAST=$(cat "$STATE" 2>/dev/null || echo 0)
case "$LAST" in *[!0-9]*|'') LAST=0;; esac
# log rotated/truncated -> rescan from start
[ "$LAST" -gt "$SIZE" ] && LAST=0

NEW=$(tail -c +"$((LAST + 1))" "$LOG")
echo "$SIZE" > "$STATE"

# strip ANSI color codes before matching
ERRORS=$(printf '%s\n' "$NEW" | sed 's/\x1b\[[0-9;]*m//g' | grep -E ' ERROR ' || true)
WARNS=$(printf '%s\n' "$NEW" | sed 's/\x1b\[[0-9;]*m//g' | grep -E ' WARN ' || true)

if [ -n "$ERRORS" ]; then
  echo "ALERT: new ERROR lines since last check:"
  printf '%s\n' "$ERRORS" | tail -20
  [ "$FAIL" -eq 0 ] && FAIL=1
else
  echo "errors: none new"
fi
if [ -n "$WARNS" ]; then
  echo "warnings (new):"
  printf '%s\n' "$WARNS" | tail -10
fi

echo "=== WATCH_DONE exit=$FAIL ==="
exit "$FAIL"
