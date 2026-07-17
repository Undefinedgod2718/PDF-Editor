# 部署（雙 OS）

系統支援兩種部署方式：**Windows（NSSM 服務）** 與 **Linux（systemd user unit）**。
兩者都在開發機執行對應腳本，經 SSH 佈署到目標主機。

## 共通前置

```sh
cd web && npm ci && npm run build     # 產出 web/dist
```

秘密一律不進 git：密碼放環境變數（`SSHPASS`）或 `deploy/.env`（已 gitignore，
範本見 `deploy/.env.example`）。

## Windows：`deploy/windows/deploy.sh`

- 模式：本機交叉/原生建置 `pdf-editor-server.exe`，連同 `pdfium.dll`、字型、
  `web/dist` 打包上傳；遠端以 [NSSM](https://nssm.cc/) 掛 Windows 服務並開防火牆埠。
- 前置：本機已有 `server/target/release/pdf-editor-server.exe`、`server/pdfium.dll`；
  首次部署需將 `nssm.exe` 放在 `deploy/windows/`（或 `deploy/`）。
- 認證：SSH 金鑰，或 `SSH_ASKPASS` 腳本。

```sh
HOST=user@192.168.17.56 REMOTE_DIR='C:\PDFEditor' SERVICE=PDFEditor PORT=8050 \
  deploy/windows/deploy.sh
```

## Linux：`deploy/linux/deploy.sh`

- 模式：上傳 server 原始碼 + `web/dist` + 字型 + Python sidecar，遠端
  `cargo build --release`，自動抓 `libpdfium.so`（bblanchon/pdfium-binaries）、
  建 Python venv（pdf2docx/pdfplumber/openpyxl），掛 `systemd --user` 服務
  `pdf-editor.service`（含 linger，開機自啟）。
- 前置：遠端有 Rust toolchain 與 python3；apt 相依（build-essential、python3-venv）
  會在有 passwordless sudo 時自動補裝。
- 認證：SSH 金鑰直接跑；或 `export SSHPASS='…'` 改走 sshpass 密碼登入。

```sh
HOST=richard@192.168.17.56 REMOTE_APP=/mnt/d/DockerRoot/pdf-editor PORT=8050 \
  deploy/linux/deploy.sh
```

## 驗證

兩個腳本結尾都會打 `GET /api/documents` 期望 HTTP 200；Linux 腳本另外檢查
`GET /api/health` 的 `sidecar.ok`（false = docx/xlsx 匯出壞掉）。

## 其他檔案

- `_wsl_*.sh`、`on56_*.sh`：既有主機（192.168.17.56）的維運小工具
  （開埠、看 log、probe），非部署主流程。
- `.env.example`：`deploy/.env` 範本。
