# 後端（server/）

本頁導覽 `server/` 的模組職責、關鍵型別、環境變數與建置注意事項。

## 模組導覽

### `server/src/main.rs`

程式進入點。職責：

- 初始化 `tracing_subscriber`（`RUST_LOG` 環境變數控制，預設 `"info,tower_http=info"`）
- 建立 `storage::Storage`（讀 `PDF_EDITOR_DATA`）與 `pdf::engine::PdfEngine::spawn()`
- 組成 `AppState { storage, engine }`，以 `SharedState = Arc<AppState>` 分享給所有 handler
- 建立 Axum `Router`：`api::router()` 掛 `/api/*`，`.fallback_service(ServeDir(web_dist).fallback(ServeFile(index.html)))` 服務前端靜態檔（`PDF_EDITOR_WEB`，預設 `../web/dist`），並加上 `CorsLayer::permissive()`
- 依 `PDF_EDITOR_PORT`（預設 `8050`）在 `0.0.0.0` 上 bind 並 `axum::serve`

### `server/src/api/mod.rs`

所有 HTTP 路由與 handler。關鍵型別：`ApiError(StatusCode, String)`，`IntoResponse` 實作會回傳 `{ "error": "..." }` 的 JSON；`From<anyhow::Error>` 一律映射為 `500`。完整 endpoint 清單見 [API.md](API.md)。

### `server/src/pdf/engine.rs`

`PdfEngine`：把所有 PDFium 呼叫序列化到單一 worker 執行緒（因 PDFium 非執行緒安全），透過 `mpsc::UnboundedSender<Job>` 送任務、`oneshot::channel` 取回結果。詳細原理見 [Architecture.md](Architecture.md#為何需要-pdfium-worker-thread)。

### `server/src/pdf/ops.rs`

實際的 PDFium 操作與其回傳型別（皆為 `#[derive(Serialize)]`）：

| 函式 | 用途 | 主要回傳型別 |
|------|------|--------------|
| `doc_info(pdfium, path)` | 頁數、標題、每頁尺寸 | `DocInfo { page_count, title, pages: Vec<PageInfo> }` |
| `render_page(pdfium, path, index, scale)` | 渲染單頁為 PNG bytes | `Vec<u8>` |
| `page_text(pdfium, path, index)` | 抽取單頁文字層（含每字元座標） | `PageText { text, chars: Vec<CharBox> }` |
| `search(pdfium, path, query)` | 全文不分大小寫搜尋，合併同行相鄰字元矩形 | `Vec<SearchHit>` |

共用型別：`Rect { x, y, w, h }`（points，左上原點，由 `to_top_left()` 從 PDFium 的左下原點座標換算）、`CharBox { c, #[serde(flatten)] rect }`、`PageInfo { index, width, height }`。

### `server/src/pdf/annots.rs`（Phase 2）

註解的建立 / 列出 / 刪除。所有跨 API 邊界的座標都是「PDF points、左上原點」，PDFium 內部用左下原點，故每個 rect/point 進出都要翻轉（`to_pdf_rect()` / `from_pdf_rect()`）。

**原子存檔模式（`with_document`）**：`create`/`delete` 都透過 `with_document(pdfium, path, |doc| { ... })` 執行：

1. `std::fs::read(path)` 把整份 PDF 讀進記憶體（`load_pdf_from_byte_vec`），刻意不用 `load_pdf_from_file`，讓過程中不持有檔案 handle
2. 呼叫傳入的 closure 對 `doc` 做修改
3. `doc.save_to_bytes()` 存成記憶體 buffer，`drop(doc)` 釋放
4. 寫到 `{path}.pdf.tmp`，再 `std::fs::rename()` 覆蓋原檔（rename 在同一磁碟區是原子操作，避免寫到一半被讀到壞檔）

**三個 PDFium 限制與對應實作決策**（完整說明見檔案頂部模組文件註解）：

| 限制 | 實作決策 |
|------|----------|
| `FreeText` subtype PDFium 不會產生 appearance stream，直接建會在渲染結果中「隱形」 | 文字框改用 `Stamp` subtype 承載一個個 `PdfPageTextObject`（`create_stamp_annotation()` + `objects_mut().add_text_object()`），因為 PDFium 只允許對 `Ink`／`Stamp` 兩種 subtype append page object |
| `Ink` 若不手動塞物件，同樣沒有可靠的 appearance | 每條筆畫轉成一個 `PdfPagePathObject`（`line_to()` 逐點連線），append 進 annotation（`objects_mut().add_path_object()`） |
| `Text`（便籤／sticky note）*有* PDFium 自動產生的 appearance | 便籤不需額外塞物件，只需 `create_text_annotation()` + `set_bounds()` + `set_stroke_color()`；圖示在前端另外疊加繪製（見 [Frontend.md](Frontend.md#annotlayertsx)） |

**quad points 順序**：四種文字標記（`Highlight`/`Underline`/`Strikeout`/`Squiggly`）共用 `setup_markup!` 巨集，手動組出符合 PDF 規範順序（左上、右上、左下、右下）的 `PdfQuadPoints`，因為 `pdfium-render` 的 `PdfQuadPoints::from_rect()` 產出 BL,BR,TR,TL 順序，PDFium 的 appearance-stream 產生器會拒絕，導致標記完全不渲染。

**渲染端配合**：`ops.rs::render_page()` 的 `PdfRenderConfig` 已開 `.render_annotations(true)`（連同既有的 `.render_form_data(true)`），註解建立/刪除後渲染出的 PNG 會直接含註解外觀，不需前端另外疊圖層繪製最終效果（前端 overlay 只用於互動期間的即時預覽，見 Frontend.md）。

**七種 `NewAnnotation` variant**：`Highlight`/`Underline`/`Strikeout`/`Squiggly`（`rects: Vec<InRect>` + `color` + 選填 `contents`）、`Note`（`x,y,contents,color`，固定 `NOTE_SIZE = 20.0` pt 圖示範圍）、`Ink`（`strokes: Vec<Vec<InPoint>>` + `color` + `width`）、`FreeText`（`rect,contents,color`，選填 `font_size` 預設 `12.0`，只支援內建 Helvetica、不支援中文）。完整 JSON 範例見 [API.md](API.md#註解-endpoint-phase-2)。

### `server/src/storage.rs`

`Storage`：文件中繼資料管理。

- `DocMeta { id: Uuid, filename: String, size: u64 }`
- 記憶體用 `DashMap<Uuid, DocMeta>`，落地用「每份文件一個 `{id}.pdf` + 一個 `{id}.json`」sidecar，`Storage::new()` 啟動時掃描資料夾重建索引，讓文件在伺服器重啟後仍可用
- `save()` 寫入 PDF 檔與 metadata JSON；`get()`/`list()`/`pdf_path()` 供 API handler 查詢

## 環境變數

| 變數 | 預設 | 說明 |
|------|------|------|
| `PDF_EDITOR_PORT` | `8050` | 監聽埠 |
| `PDF_EDITOR_DATA` | `data` | 上傳檔案儲存目錄 |
| `PDF_EDITOR_WEB` | `../web/dist` | 前端靜態檔目錄 |
| `RUST_LOG` | `info,tower_http=info` | tracing 過濾器 |

## 建置注意事項

- **`CARGO_TARGET_DIR` 必須本地覆蓋**：本機環境有全域 `CARGO_TARGET_DIR` 設定會污染建置輸出路徑，開發/建置前務必先執行：
  ```powershell
  cd server
  $env:CARGO_TARGET_DIR="$PWD\target"
  cargo build   # 或 cargo run / cargo test
  ```
- **`pdfium.dll`**：執行期由 `Pdfium::bind_to_library(Pdfium::pdfium_platform_library_name_at_path("./"))` 優先載入，找不到才退回系統函式庫（`bind_to_system_library`）。因此執行檔（無論 `cargo run` 或 release build）的**工作目錄**必須含有 `pdfium.dll`（Windows x64）。此 DLL 來源為 [bblanchon/pdfium-binaries](https://github.com/bblanchon/pdfium-binaries)。
- 依賴版本（`server/Cargo.toml`）：`axum 0.8`（`multipart` feature）、`tokio`（`full`）、`tower-http 0.6`（`fs, cors, trace`）、`pdfium-render 0.8`、`image 0.25`、`dashmap 6`、`serde`/`serde_json`、`uuid`（`v4, serde`）、`tokio-util`（`io`）、`urlencoding 2`、`anyhow`、`tracing`/`tracing-subscriber`。`[profile.release]` 開啟 `lto = true` 與 `strip = true`。
- 執行 release 版本時同樣需要工作目錄含 `pdfium.dll`，且 `../web/dist` 必須存在（或設定 `PDF_EDITOR_WEB`）。

## 相關頁面

- [Architecture.md](Architecture.md) — 系統架構與渲染管線
- [API.md](API.md) — 完整 API 參考
- [Verification.md](Verification.md) — 後端驗證清單

---
最後更新：2026-07-10（Phase 2）
