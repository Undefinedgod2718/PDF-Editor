#!/bin/sh
# PDF Editor — Windows 部署（NSSM 服務）。在開發機 Git Bash / WSL 執行。
# 前置：已在本機建好 server/target/release/pdf-editor-server.exe 與 web/dist。
# 認證：SSH_ASKPASS 指向回應密碼的腳本，或已設 SSH 金鑰後直接執行。
#
# 可用環境變數覆寫（預設值 = 既有 192.168.17.56 部署）：
#   HOST        ssh 目標（user@host）
#   REMOTE_DIR  遠端安裝目錄（Windows 路徑，反斜線）
#   SERVICE     NSSM 服務名稱
#   PORT        對外埠（防火牆規則）
set -e

HOST="${HOST:-user@192.168.17.56}"
REMOTE_DIR="${REMOTE_DIR:-C:\\PDFEditor}"
SERVICE="${SERVICE:-PDFEditor}"
PORT="${PORT:-8050}"

ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
STAGE="$ROOT/deploy/stage"
# scp 需要正斜線路徑
REMOTE_DIR_FWD=$(printf '%s' "$REMOTE_DIR" | tr '\\' '/')

echo "== 打包 =="
rm -rf "$STAGE"
mkdir -p "$STAGE/web" "$STAGE/fonts"
cp "$ROOT/server/target/release/pdf-editor-server.exe" "$STAGE/"
cp "$ROOT/server/pdfium.dll" "$STAGE/"
cp "$ROOT/server/fonts/GenSenRoundedTW-R.ttf" "$STAGE/fonts/"
cp "$ROOT/server/fonts/SIL_Open_Font_License_1.1.txt" "$STAGE/fonts/"
cp -r "$ROOT/web/dist" "$STAGE/web/dist"
[ -f "$ROOT/deploy/windows/nssm.exe" ] && cp "$ROOT/deploy/windows/nssm.exe" "$STAGE/"
[ -f "$ROOT/deploy/nssm.exe" ] && cp "$ROOT/deploy/nssm.exe" "$STAGE/"

echo "== 停服務（若存在）並建目錄 =="
# 注意：ssh 無 TTY 下 `timeout /t` 會因輸入重導向失敗，改用 ping 充當 sleep
ssh "$HOST" "if exist $REMOTE_DIR\\nssm.exe ($REMOTE_DIR\\nssm.exe stop $SERVICE & ping -n 3 127.0.0.1 >nul) else (echo first-deploy) & if not exist $REMOTE_DIR mkdir $REMOTE_DIR" || true

echo "== 傳檔 =="
scp -r "$STAGE"/* "$HOST:$REMOTE_DIR_FWD/"

echo "== 設定服務 + 防火牆 =="
ssh "$HOST" "$REMOTE_DIR\\nssm.exe install $SERVICE $REMOTE_DIR\\pdf-editor-server.exe 2>nul & $REMOTE_DIR\\nssm.exe set $SERVICE AppDirectory $REMOTE_DIR & $REMOTE_DIR\\nssm.exe set $SERVICE AppEnvironmentExtra PDF_EDITOR_WEB=$REMOTE_DIR\\web\\dist PDF_EDITOR_DATA=$REMOTE_DIR\\data & $REMOTE_DIR\\nssm.exe set $SERVICE Start SERVICE_AUTO_START & $REMOTE_DIR\\nssm.exe set $SERVICE AppStdout $REMOTE_DIR\\service.log & $REMOTE_DIR\\nssm.exe set $SERVICE AppStderr $REMOTE_DIR\\service.log & netsh advfirewall firewall delete rule name=$SERVICE$PORT >nul 2>&1 & netsh advfirewall firewall add rule name=$SERVICE$PORT dir=in action=allow protocol=TCP localport=$PORT & $REMOTE_DIR\\nssm.exe restart $SERVICE"

echo "== 驗證 =="
sleep 3
HOST_IP=${HOST#*@}
curl -s -o /dev/null -w "http://$HOST_IP:$PORT -> HTTP %{http_code}\n" "http://$HOST_IP:$PORT/api/documents"
