# 部署（雙 OS）

系統支援三種部署方式：**Windows 桌面 MSI**、**Windows（NSSM 服務）** 與
**Linux（systemd user unit）**。前兩者為 Windows 上不同產物（桌面 app vs 多人
server）；Linux 腳本在開發機執行，經 SSH 佈署到目標主機。

## 共通前置

```sh
cd web && npm ci && npm run build     # 產出 web/dist
```

秘密一律不進 git：密碼放環境變數（`SSHPASS`）或 `deploy/.env`（已 gitignore，
範本見 `deploy/.env.example`）。

## Windows 桌面 MSI：`deploy/windows/build-msi.ps1`

- **產物**：Tauri WiX `.msi`（含 WebView2 bootstrapper 下載模式）。
- **建置機**：必須在 **Windows** 上執行（WiX candle/light 無法在 Linux 產 MSI）。
- **本 repo Linux 開發機**：只維護設定與腳本；實際編譯交 Windows 建置機。
- **前置**：`server/pdfium.dll`、`server/fonts/GenSenRoundedTW-R.ttf`、已 build 的
  `web/dist`；Rust + Node + WiX Toolset v3。

```powershell
# 在 repo 根目錄（PowerShell）
.\deploy\windows\build-msi.ps1
```

輸出：`desktop/target/release/bundle/msi/*.msi`

安裝精靈含「設為預設 PDF 開啟程式」勾選（預設勾選）；未勾選仍註冊 Open with。
Windows 10/11 可能仍要求在「預設應用程式」再確認一次。

## Windows 多人服務：`deploy/windows/deploy.sh`

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
