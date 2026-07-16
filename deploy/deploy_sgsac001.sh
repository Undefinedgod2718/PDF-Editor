#!/bin/bash
# Deploy PDF Editor → sgsac001 (Ubuntu). Run via WSL bash (LF), not PowerShell.
# Host: richard@192.168.17.56  |  app: /mnt/d/DockerRoot/pdf-editor  |  port: 8050
#
# Auth: export SSHPASS from ops vault (never commit). Optional: deploy/.env (gitignored).
#   export SSHPASS='…'   # or: set -a; source deploy/.env; set +a
set -euo pipefail

HOST="${HOST:-richard@192.168.17.56}"
ROOT="${ROOT:-/mnt/d/Program_Coding/PDF Editor}"

# Load optional local secrets file (not in git).
if [ -f "$ROOT/deploy/.env" ]; then
  # shellcheck disable=SC1091
  set -a
  # strip CR if edited on Windows
  eval "$(tr -d '\015' < "$ROOT/deploy/.env" | grep -E '^[A-Za-z_][A-Za-z0-9_]*=' || true)"
  set +a
fi

if [ -z "${SSHPASS:-}" ]; then
  echo "FATAL: SSHPASS unset. Export it (or put SSHPASS=… in deploy/.env). No hardcoded passwords." >&2
  exit 1
fi
export SSHPASS

SSH=(sshpass -e ssh -o StrictHostKeyChecking=no -o PreferredAuthentications=password -o PubkeyAuthentication=no)
SCP=(sshpass -e scp -o StrictHostKeyChecking=no -o PreferredAuthentications=password -o PubkeyAuthentication=no)

REMOTE_APP="/mnt/d/DockerRoot/pdf-editor"
PDFIUM_URL="https://github.com/bblanchon/pdfium-binaries/releases/download/chromium%2F7947/pdfium-linux-x64.tgz"
STAGE=$(mktemp -d /tmp/pdf-editor-stage.XXXXXX)
trap 'rm -rf "$STAGE"' EXIT

echo "== check local web/dist =="
test -f "$ROOT/web/dist/index.html"

echo "== pack source tarball =="
mkdir -p "$STAGE/pack"
tar -C "$ROOT" \
  --exclude='server/target' \
  --exclude='server/pdfium.dll' \
  --exclude='web/node_modules' \
  --exclude='web/dist' \
  --exclude='.git' \
  --exclude='deploy/stage' \
  -czf "$STAGE/pack/src.tgz" server

tar -C "$ROOT/web" -czf "$STAGE/pack/webdist.tgz" dist
tar -C "$ROOT/server/fonts" -czf "$STAGE/pack/fonts.tgz" .
tar -C "$ROOT" \
  --exclude='python/.venv' \
  --exclude='python/.testout' \
  -czf "$STAGE/pack/python.tgz" python

# Remote install — no passwords baked in. apt uses passwordless sudo (-n) or skip.
cat > "$STAGE/remote_install.sh" <<REMOTE
#!/bin/bash
set -euo pipefail
export PATH="/mnt/c/tools/bin:\$HOME/.cargo/bin:\$PATH"
APP=$REMOTE_APP
BIN=\$APP/bin
DATA=\$APP/data
WEB=\$APP/web/dist
SRC=\$APP/src
PDFIUM_URL='$PDFIUM_URL'

mkdir -p "\$BIN" "\$DATA" "\$WEB" "\$SRC" "\$APP/fonts" "\$APP/logs" "\$BIN/fonts"

echo "== unpack =="
tar -xzf /tmp/pdf-editor-src.tgz -C "\$SRC"
tar -xzf /tmp/pdf-editor-webdist.tgz -C "\$APP/web"
tar -xzf /tmp/pdf-editor-fonts.tgz -C "\$APP/fonts"
cp -a "\$APP/fonts/." "\$BIN/fonts/"
tar -xzf /tmp/pdf-editor-python.tgz -C "\$APP"

echo "== python sidecar venv (docx/xlsx conversion) =="
PYVENV="\$APP/python/.venv"
if [ ! -x "\$PYVENV/bin/python" ]; then
  if ! python3 -m venv "\$PYVENV" 2>/dev/null; then
    if sudo -n true 2>/dev/null; then
      sudo -n DEBIAN_FRONTEND=noninteractive apt-get install -y -qq python3-venv python3-pip
      python3 -m venv "\$PYVENV"
    else
      echo "FATAL: python3 -m venv failed and passwordless sudo unavailable. Install python3-venv manually." >&2
      exit 1
    fi
  fi
fi
"\$PYVENV/bin/pip" install --quiet --upgrade pip
"\$PYVENV/bin/pip" install --quiet 'pdf2docx>=0.5.13' 'pdfplumber>=0.11.10' 'openpyxl>=3.1.5'
"\$PYVENV/bin/python" -c 'import pdf2docx, pdfplumber, openpyxl' \
  || { echo "FATAL: sidecar deps import failed" >&2; exit 1; }

echo "== pdfium =="
if [ ! -f "\$BIN/libpdfium.so" ]; then
  TMP=\$(mktemp -d)
  curl -fsSL -o "\$TMP/pdfium.tgz" "\$PDFIUM_URL"
  tar -xzf "\$TMP/pdfium.tgz" -C "\$TMP"
  SO=\$(find "\$TMP" -name 'libpdfium.so' | head -1)
  test -n "\$SO"
  cp "\$SO" "\$BIN/libpdfium.so"
  rm -rf "\$TMP"
fi

echo "== apt build deps (idempotent) =="
if ! command -v cc >/dev/null 2>&1; then
  if sudo -n true 2>/dev/null; then
    sudo -n apt-get update -qq
    sudo -n DEBIAN_FRONTEND=noninteractive apt-get install -y -qq build-essential pkg-config
  else
    echo "FATAL: no C compiler and passwordless sudo unavailable. Install build-essential manually." >&2
    exit 1
  fi
fi

echo "== cargo build --release =="
cd "\$SRC/server"
unset CARGO_TARGET_DIR || true
export CARGO_TARGET_DIR="\$SRC/server/target"
cargo build --release

cp -f "\$SRC/server/target/release/pdf-editor-server" "\$BIN/pdf-editor-server"
chmod +x "\$BIN/pdf-editor-server"

echo "== systemd user unit =="
mkdir -p "\$HOME/.config/systemd/user"
cat > "\$HOME/.config/systemd/user/pdf-editor.service" <<EOF
[Unit]
Description=PDF Editor (sgsac001)
After=network.target

[Service]
Type=simple
WorkingDirectory=\$BIN
Environment=PDF_EDITOR_PORT=8050
Environment=PDF_EDITOR_DATA=\$DATA
Environment=PDF_EDITOR_WEB=\$WEB
Environment=PDF_EDITOR_PYTHON=\$APP/python/.venv/bin/python
Environment=PDF_EDITOR_SIDECAR=\$APP/python/convert.py
Environment=LD_LIBRARY_PATH=\$BIN
ExecStart=\$BIN/pdf-editor-server
Restart=on-failure
RestartSec=3
StandardOutput=append:\$APP/logs/service.log
StandardError=append:\$APP/logs/service.log

[Install]
WantedBy=default.target
EOF

systemctl --user daemon-reload
systemctl --user enable pdf-editor.service
systemctl --user restart pdf-editor.service
loginctl enable-linger "\$(whoami)" 2>/dev/null || true

sleep 2
systemctl --user --no-pager --full status pdf-editor.service || true
ss -lnt | grep 8050 || true
curl -s -o /dev/null -w "local_api=%{http_code}\\n" http://127.0.0.1:8050/api/documents || true
echo "== health (sidecar must be ok:true) =="
curl -s http://127.0.0.1:8050/api/health || true
echo
echo REMOTE_INSTALL_OK
REMOTE

echo "== upload =="
"${SSH[@]}" "$HOST" "mkdir -p $REMOTE_APP/web $REMOTE_APP/bin $REMOTE_APP/data $REMOTE_APP/logs $REMOTE_APP/fonts $REMOTE_APP/src"
"${SCP[@]}" "$STAGE/pack/src.tgz" "$HOST:/tmp/pdf-editor-src.tgz"
"${SCP[@]}" "$STAGE/pack/webdist.tgz" "$HOST:/tmp/pdf-editor-webdist.tgz"
"${SCP[@]}" "$STAGE/pack/fonts.tgz" "$HOST:/tmp/pdf-editor-fonts.tgz"
"${SCP[@]}" "$STAGE/pack/python.tgz" "$HOST:/tmp/pdf-editor-python.tgz"
"${SCP[@]}" "$STAGE/remote_install.sh" "$HOST:/tmp/pdf-editor-remote_install.sh"

echo "== remote install =="
"${SSH[@]}" "$HOST" "tr -d '\015' < /tmp/pdf-editor-remote_install.sh > /tmp/pdf-editor-ri.sh && bash /tmp/pdf-editor-ri.sh"

echo "== console verify =="
sleep 1
curl -s -o /dev/null -w "http://192.168.17.56:8050/api/documents -> HTTP %{http_code}\n" \
  http://192.168.17.56:8050/api/documents
echo DEPLOY_DONE
