# PDF Editor

仿 Adobe Acrobat DC 的 Web PDF 編輯器。Rust 後端 + React 前端，單埠部署。

## 架構

- `server/` — Rust (Axum) 後端：PDF 上傳、PDFium 渲染、文字層、搜尋 API，並服務前端靜態檔
- `web/` — React 18 + TypeScript (Vite) 前端：仿 Acrobat 檢視器 UI
- 渲染引擎：PDFium（`server/pdfium.dll`，Windows x64）
- 埠：8050（可用環境變數 `PDF_EDITOR_PORT` 覆蓋）

## 開發

```powershell
# 後端（首次需將 pdfium.dll 放在 server/ 或工作目錄）
cd server
$env:CARGO_TARGET_DIR="$PWD\target"   # 本機有全域 CARGO_TARGET_DIR 時需覆蓋
cargo run

# 前端（開發模式，proxy /api → 127.0.0.1:8050）
cd web
npm run dev
```

## 建置與執行

```powershell
cd web && npm run build        # 產出 web/dist
cd server && cargo build --release
# 執行：工作目錄需含 pdfium.dll，且 ../web/dist 存在（或設 PDF_EDITOR_WEB）
.\target\release\pdf-editor-server.exe
```

瀏覽器開 http://localhost:8050

## 環境變數

| 變數 | 預設 | 說明 |
|------|------|------|
| `PDF_EDITOR_PORT` | `8050` | 監聽埠 |
| `PDF_EDITOR_DATA` | `data` | 上傳檔案儲存目錄 |
| `PDF_EDITOR_WEB` | `../web/dist` | 前端靜態檔目錄 |

## API

| 方法 | 路徑 | 說明 |
|------|------|------|
| POST | `/api/documents` (multipart `file`) | 上傳 PDF |
| GET | `/api/documents` | 列出文件 |
| GET | `/api/documents/{id}/info` | 頁數、頁面尺寸(pt)、標題 |
| GET | `/api/documents/{id}/pages/{n}/render?scale=` | 頁面 PNG（scale = px/pt，1.0 = 72dpi） |
| GET | `/api/documents/{id}/pages/{n}/text` | 文字層（字元 + 座標，左上原點，pt） |
| GET | `/api/documents/{id}/search?q=` | 全文搜尋（不分大小寫），回傳頁碼與命中矩形 |
| GET | `/api/documents/{id}/download` | 下載原檔 |
| POST | `/api/documents/{id}/pages/{page}/annotations` | 新增註解（螢光標記/底線/刪除線/波浪線/便籤/手繪/文字框，七種 `type`） |
| GET | `/api/documents/{id}/pages/{page}/annotations` | 列出該頁註解 |
| DELETE | `/api/documents/{id}/pages/{page}/annotations/{index}` | 刪除單筆註解 |

## 開發階段

- [x] Phase 1：檢視器（渲染、翻頁、縮放、縮圖、搜尋）
- [x] Phase 2：註解（螢光標記、底線、刪除線、波浪線、便籤、手繪、文字框）
- [ ] Phase 3：內容編輯（文字/圖片、頁面增刪旋轉、合併分割）
- [ ] Phase 4：表單填寫/建立、電子簽名
- [ ] Phase 5：部署至 192.168.17.56:8050（Windows，NSSM 服務）
