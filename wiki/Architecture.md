# 系統架構

本頁說明 PDF Editor 的整體系統架構、後端 PDFium worker thread 設計、以及渲染管線與座標系。

## 總覽

```
瀏覽器
  │  HTTP（單埠 8050）
  ▼
Rust (Axum) 後端  ── ServeDir(../web/dist) 服務前端靜態檔（fallback → index.html）
  │
  ├─ api/mod.rs        路由與 handler（上傳、資訊、渲染、文字層、搜尋、下載）
  ├─ storage.rs         文件中繼資料 + 檔案儲存（記憶體 DashMap + JSON sidecar 落地）
  └─ pdf/engine.rs      PDFium worker thread（mpsc + oneshot）
        └─ pdf/ops.rs   實際 PDFium 操作（doc_info / render_page / page_text / search）

React (Vite) 前端
  └─ 透過 /api/* 呼叫後端，開發模式由 vite proxy 轉發到 127.0.0.1:8050
```

## 單埠部署模式

Rust 伺服器（`server/src/main.rs`）同時扮演 API 伺服器與靜態檔伺服器：`api::router()` 掛載所有 `/api/*` 路由，`.fallback_service(static_files)` 則將其餘請求交給 `ServeDir::new(web_dist).fallback(ServeFile::new(index.html))`，因此正式環境只需啟動一個行程、監聽一個埠（預設 `8050`，見 [Backend.md](Backend.md) 的環境變數表）即可同時提供前後端。

## 為何需要 PDFium worker thread

PDFium（透過 `pdfium-render` crate 綁定）並非執行緒安全，不能在多個 async task 或多執行緒中並行呼叫。`server/src/pdf/engine.rs` 因此把所有 PDFium 操作集中到一條專屬的 OS 執行緒（`pdfium-worker`）上執行：

- `PdfEngine::spawn()` 建立該執行緒，並在其中一次性完成 `Pdfium::bind_to_library`（先嘗試載入同目錄下的 `pdfium.dll`，失敗則退回系統函式庫）與 `Pdfium::new(bindings)`。
- 呼叫端（Axum handler）透過 `PdfEngine::run(f)` 把一個 `FnOnce(&Pdfium) -> anyhow::Result<T>` 閉包，包裝進 `Job`（`Box<dyn FnOnce(&Pdfium) + Send>`）送進 `mpsc::UnboundedSender`。
- worker 執行緒用 `rx.blocking_recv()` 依序取出並執行 job，結果透過對應的 `oneshot::channel` 送回呼叫端的 `await`。

這個設計確保任何時刻只有一個執行緒在碰 PDFium，同時讓 Axum 的 async handler 可以用 `.await` 的方式取得結果，不必自己管理鎖或執行緒同步。

## 渲染管線

1. 前端請求 `GET /api/documents/{id}/pages/{page}/render?scale=`（`scale` = 每 PDF point 對應的像素數，`1.0` = 72 dpi）。
2. Handler（`api/mod.rs::render_page`）把 `scale` clamp 到 `0.1–8.0`，透過 `engine.run()` 呼叫 `pdf::ops::render_page`。
3. `ops::render_page` 用 `pdfium_render` 載入該頁、依 `width = page.width * scale` 設定 `PdfRenderConfig`（並開啟 `render_form_data`），渲染成 bitmap 後編碼為 PNG bytes 回傳。
4. Handler 回應 `Content-Type: image/png`、`Cache-Control: no-store` 的二進位串流。

前端（`web/src/components/Viewer.tsx`）以 CSS 尺寸 `page.width * scale` 顯示頁面，但實際請求渲染時用 `scale * devicePixelRatio`（上限 2）以取得更銳利的畫面，細節見 [Frontend.md](Frontend.md)。

## 座標系

對外 API 一律使用「PDF points、左上原點」座標系：

- PDFium 原生座標是「points、左下原點」；`pdf/ops.rs::to_top_left()` 負責把 PDFium 回傳的 `PdfRect`（`left/top/right/bottom`）轉換成 `{ x, y, w, h }`，其中 `y` 是「距頁面頂端」的距離。
- 文字層（`page_text`）與搜尋結果（`search`）的每個字元/命中矩形都使用這個座標系，前端只需乘上目前的 `scale` 即可疊加定位（見 `Viewer.tsx` 內 `hl` 高亮 div 的 `left/top/width/height` 計算）。
- 頁面尺寸（`PageInfo.width/height`，來自 `doc_info`）同樣是 points，未經 DPI 縮放。

## 相關頁面

- [Backend.md](Backend.md) — 後端模組細節、環境變數、建置注意事項
- [Frontend.md](Frontend.md) — 前端元件與資料流
- [API.md](API.md) — 完整 API 參考

---
最後更新：2026-07-10（Phase 1）
