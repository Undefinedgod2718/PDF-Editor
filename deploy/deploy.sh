#!/bin/sh
# PDF Editor 部署腳本（在開發機 Git Bash 執行）
# 目標：user@192.168.17.56 → C:\PDFEditor，NSSM 服務 PDFEditor，埠 8050
# 用法：SSH_ASKPASS 指向回應密碼的腳本，或已設 SSH 金鑰後直接執行
set -e

HOST=user@192.168.17.56
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
STAGE="$ROOT/deploy/stage"

echo "== 打包 =="
rm -rf "$STAGE"
mkdir -p "$STAGE/web" "$STAGE/fonts"
cp "$ROOT/server/target/release/pdf-editor-server.exe" "$STAGE/"
cp "$ROOT/server/pdfium.dll" "$STAGE/"
cp "$ROOT/server/fonts/GenSenRoundedTW-R.ttf" "$STAGE/fonts/"
cp "$ROOT/server/fonts/SIL_Open_Font_License_1.1.txt" "$STAGE/fonts/"
cp -r "$ROOT/web/dist" "$STAGE/web/dist"
[ -f "$ROOT/deploy/nssm.exe" ] && cp "$ROOT/deploy/nssm.exe" "$STAGE/"

echo "== 停服務（若存在）並建目錄 =="
# 注意：ssh 無 TTY 下 `timeout /t` 會因輸入重導向失敗，改用 ping 充當 sleep
ssh "$HOST" "if exist C:\\PDFEditor\\nssm.exe (C:\\PDFEditor\\nssm.exe stop PDFEditor & ping -n 3 127.0.0.1 >nul) else (echo first-deploy) & if not exist C:\\PDFEditor mkdir C:\\PDFEditor" || true

echo "== 傳檔 =="
scp -r "$STAGE"/* "$HOST:C:/PDFEditor/"

echo "== 設定服務 + 防火牆 =="
ssh "$HOST" "C:\\PDFEditor\\nssm.exe install PDFEditor C:\\PDFEditor\\pdf-editor-server.exe 2>nul & C:\\PDFEditor\\nssm.exe set PDFEditor AppDirectory C:\\PDFEditor & C:\\PDFEditor\\nssm.exe set PDFEditor AppEnvironmentExtra PDF_EDITOR_WEB=C:\\PDFEditor\\web\\dist PDF_EDITOR_DATA=C:\\PDFEditor\\data & C:\\PDFEditor\\nssm.exe set PDFEditor Start SERVICE_AUTO_START & C:\\PDFEditor\\nssm.exe set PDFEditor AppStdout C:\\PDFEditor\\service.log & C:\\PDFEditor\\nssm.exe set PDFEditor AppStderr C:\\PDFEditor\\service.log & netsh advfirewall firewall delete rule name=PDFEditor8050 >nul 2>&1 & netsh advfirewall firewall add rule name=PDFEditor8050 dir=in action=allow protocol=TCP localport=8050 & C:\\PDFEditor\\nssm.exe restart PDFEditor"

echo "== 驗證 =="
sleep 3
curl -s -o /dev/null -w "http://192.168.17.56:8050 -> HTTP %{http_code}\n" http://192.168.17.56:8050/api/documents
