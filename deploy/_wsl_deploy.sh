#!/bin/bash
set -euo pipefail
export ROOT="/mnt/d/Program_Coding/PDF Editor"
SRC="$ROOT/deploy/deploy_sgsac001.sh"
tr -d '\015' < "$SRC" > /tmp/deploy_sgsac001.sh
chmod +x /tmp/deploy_sgsac001.sh
bash /tmp/deploy_sgsac001.sh
