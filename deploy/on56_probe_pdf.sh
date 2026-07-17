#!/bin/bash
set -euo pipefail
echo "=== host ==="
hostname
whoami
uname -a
echo "=== mounts ==="
findmnt -no SOURCE,UUID,FSTYPE /
findmnt -no SOURCE,UUID,FSTYPE /mnt/d || echo "NO_/mnt/d"
echo "=== tools ==="
export PATH="/mnt/c/tools/bin:$HOME/.cargo/bin:$PATH"
command -v cargo || true
command -v rustc || true
ls /mnt/c/tools/bin 2>/dev/null | head -30 || true
ls "$HOME/.cargo/bin" 2>/dev/null | head -20 || true
echo "=== dockerroot ==="
ls /mnt/d/DockerRoot 2>/dev/null || echo "NO_DockerRoot"
echo "=== ports ==="
ss -lnt | grep -E ':8050|:8080|:8081' || true
echo "=== disk ==="
df -h / /mnt/d | sed -n '1,5p'
echo "=== PROBE_OK ==="
