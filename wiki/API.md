# API 參考

本頁是後端所有 HTTP endpoint 的完整參考，欄位名稱依 `server/src/api/mod.rs` 與 `server/src/pdf/ops.rs` 的實際 `Serialize` 結構列出（注意大小寫風格並不統一，以下照程式碼實況標示）。

所有路由定義於 `server/src/api/mod.rs::router()`，掛載在 `/api/*` 下。錯誤一律回傳 `{ "error": "訊息" }`（`ApiError`），狀態碼視情況為 `400 / 404 / 422 / 500`。

## `POST /api/documents`

上傳 PDF。`multipart/form-data`，欄位名須為 `file`。

- 檔案內容必須以 `%PDF` 開頭，否則回 `422 Unprocessable Entity`
- 缺少 `file` 欄位回 `400 Bad Request`

**回應**（`DocMeta`，欄位無 rename，皆為原始蛇形/單字，故與 camelCase 無異）：

```json
{
  "id": "5f9c2e3a-1234-4a5b-8c9d-abcdef123456",
  "filename": "sample.pdf",
  "size": 102400
}
```

## `GET /api/documents`

列出所有已上傳文件。

**回應**：`DocMeta` 陣列，元素結構同上傳回應。

## `GET /api/documents/{id}/info`

取得文件資訊。`id` 為 `Uuid`；查無文件回 `404`。

**回應**（handler 手動組出的 JSON，注意 `pageCount` 是刻意用 camelCase 命名，其餘欄位沿用原始 struct 命名）：

```json
{
  "id": "5f9c2e3a-1234-4a5b-8c9d-abcdef123456",
  "filename": "sample.pdf",
  "size": 102400,
  "pageCount": 3,
  "title": "文件標題或 null",
  "pages": [
    { "index": 0, "width": 612.0, "height": 792.0 },
    { "index": 1, "width": 612.0, "height": 792.0 },
    { "index": 2, "width": 612.0, "height": 792.0 }
  ]
}
```

`pages[].width/height` 為 PDF points（未縮放）。`title` 來自 PDF metadata 的 `Title` tag，找不到則為 `null`。

## `GET /api/documents/{id}/pages/{page}/render`

渲染單頁為 PNG 圖片。

**查詢參數**：

| 參數 | 型別 | 預設 | 說明 |
|------|------|------|------|
| `scale` | `f32` | `1.5` | 每 PDF point 的像素數（`1.0` = 72 dpi），伺服器端會 clamp 到 `0.1–8.0` |

**路徑參數**：`id`（`Uuid`）、`page`（`u16`，0-based 頁碼索引）。

**回應**：`image/png` 二進位內容，`Cache-Control: no-store`。

## `GET /api/documents/{id}/pages/{page}/text`

取得單頁文字層（每字元含座標）。

**回應**（`PageText`，欄位無 rename，直接照 struct 命名；`CharBox` 用 `#[serde(flatten)]` 展開 `Rect`）：

```json
{
  "text": "Hello 世界",
  "chars": [
    { "c": "H", "x": 72.0, "y": 100.0, "w": 8.5, "h": 12.0 },
    { "c": "e", "x": 80.5, "y": 100.0, "w": 7.2, "h": 12.0 }
  ]
}
```

座標系：PDF points，**左上原點**（後端在 `pdf/ops.rs::to_top_left()` 已把 PDFium 原生的左下原點座標換算過）。

## `GET /api/documents/{id}/search`

全文搜尋（不分大小寫的子字串比對），回傳每個命中的頁碼與（合併同行相鄰字元後的）矩形。

**查詢參數**：`q`（字串，必填；空字串回傳空陣列）。

**回應**（`Vec<SearchHit>`，欄位無 rename）：

```json
[
  {
    "page": 0,
    "rects": [
      { "x": 72.0, "y": 100.0, "w": 40.0, "h": 12.0 }
    ],
    "excerpt": "...前後各約 20 字元的上下文，含命中字串..."
  }
]
```

- `page`：0-based 頁碼索引
- `rects`：命中文字的矩形列表（同行相鄰字元已合併，可能跨行故有多個矩形），座標系同文字層（points、左上原點）
- `excerpt`：命中處前後各約 20 個字元組成的摘要文字

## `GET /api/documents/{id}/download`

下載原始 PDF 檔案。

**回應**：`Content-Type: application/pdf`，`Content-Disposition: attachment; filename*=UTF-8''<url-encoded 檔名>`，串流回傳檔案內容。查無文件回 `404`。

## 註解 endpoint（Phase 2）

型別定義在 `server/src/pdf/annots.rs`。座標系與其他 endpoint 一致：**PDF points、左上原點**（後端內部會換算成 PDFium 的左下原點）。

### `POST /api/documents/{id}/pages/{page}/annotations`

依 `Content-Type: application/json` body 的 `type` 欄位建立一筆註解（`NewAnnotation`，`#[serde(tag = "type", rename_all = "camelCase")]`）。共七種 `type`，四種文字標記共用 `rects` 陣列：

**`highlight` / `underline` / `strikeout` / `squiggly`**（螢光標記／底線／刪除線／波浪線）：

```json
{
  "type": "highlight",
  "rects": [{ "x": 72.0, "y": 100.0, "w": 40.0, "h": 12.0 }],
  "color": { "r": 255, "g": 214, "b": 0, "a": 150 },
  "contents": "可選的備註文字"
}
```

- `rects` 不可為空陣列，否則 500（`markup annotation needs at least one rect`）
- `color.a`（透明度，0–255）為選填，預設 `255`（`opaque()`）
- `contents` 為選填

**`note`**（便籤，PDFium `Text` subtype）：

```json
{ "type": "note", "x": 72.0, "y": 100.0, "contents": "備註內容", "color": { "r": 255, "g": 214, "b": 0 } }
```

- 固定產生 20×20 pt 的圖示範圍（`NOTE_SIZE`），`x/y` 為左上角座標

**`ink`**（手繪，可一次帶多條筆畫）：

```json
{
  "type": "ink",
  "strokes": [[{ "x": 72.0, "y": 100.0 }, { "x": 80.0, "y": 110.0 }, { "x": 90.0, "y": 105.0 }]],
  "color": { "r": 0, "g": 0, "b": 0 },
  "width": 2.0
}
```

- 每條筆畫至少需 2 個點；若所有筆畫都少於 2 點則 500（`ink annotation needs at least one stroke with 2+ points`）
- `width` 為線寬（points）

**`freeText`**（文字框；後端以 `Stamp` subtype 儲存，見下方重要事實）：

```json
{
  "type": "freeText",
  "rect": { "x": 72.0, "y": 100.0, "w": 200.0, "h": 60.0 },
  "contents": "多行文字\n第二行",
  "color": { "r": 0, "g": 0, "b": 0 },
  "fontSize": 14
}
```

- `fontSize` 選填，預設 `12.0`（`default_font_size()`）；超出 `rect` 高度的文字行會被截斷（不換行 wrap，只依 `\n` 分行）
- 目前只用內建 Helvetica 字型（`doc.fonts_mut().helvetica()`），不支援中文字元

**回應**：`{ "count": <該頁註解總數 usize> }`（是建立後全頁的註解數量，**不是**新註解的 index）。

### `GET /api/documents/{id}/pages/{page}/annotations`

列出該頁所有註解（`AnnotationInfo` 陣列）。

```json
[
  { "index": 0, "type": "Highlight", "rect": { "x": 72.0, "y": 100.0, "w": 40.0, "h": 12.0 }, "contents": null },
  { "index": 1, "type": "Stamp", "rect": { "x": 72.0, "y": 200.0, "w": 200.0, "h": 60.0 }, "contents": "多行文字\n第二行" }
]
```

**重要事實 — `type` 是 PDFium annotation subtype 的大寫命名（`format!("{:?}", annot.annotation_type())`），不是建立時送出的 camelCase `type`**：

| 建立時送出的 `type` | GET 回傳的 `type` |
|---|---|
| `highlight` | `Highlight` |
| `underline` | `Underline` |
| `strikeout` | `StrikeOut`（注意大寫 O） |
| `squiggly` | `Squiggly` |
| `note` | `Text` |
| `ink` | `Ink` |
| `freeText` | `Stamp`（見下方說明） |

- `rect`：`None`（PDFium 取不到 bounds 時）序列化為 `null`
- `contents`：無備註/文字內容時為 `null`

### `DELETE /api/documents/{id}/pages/{page}/annotations/{index}`

依 `list` 回傳的 `index` 刪除單筆註解。

**回應**：`{ "ok": true }`。**注意**：`index` 超出範圍時目前回 `500`（`annots::delete` 內部用 `anyhow::anyhow!` 包裝，未特化成 `404`），並非直覺的 404，呼叫端需自行處理。

### 重要實作事實（詳見 `server/src/pdf/annots.rs` 模組文件註解與 [Backend.md](Backend.md#serversrcpdfannotsrsphase-2)）

- **`freeText` 存成 `Stamp` annotation**：PDFium 不會為 `FreeText` subtype 產生 appearance stream，直接用會在渲染結果中消失；改用 `Stamp`（可 append page objects 的兩種 subtype之一）承載文字物件，才能被渲染出來。
- **quad points 必須用 PDF 規範順序**（左上、右上、左下、右下）：`pdfium-render` 的 `PdfQuadPoints::from_rect()` 產出的順序是 BL,BR,TR,TL，PDFium 的 appearance-stream 產生器不接受，會導致標記完全不渲染；後端在 `setup_markup!` 巨集中改為手動組出正確順序的 `PdfQuadPoints`。

## 前端呼叫封裝

`web/src/api.ts` 對應封裝：`uploadPdf()`、`fetchDocInfo()`、`renderUrl()`（回傳渲染 URL，不直接 fetch）、`searchDoc()`、`downloadUrl()`（回傳下載 URL）、`createAnnotation()` / `listAnnotations()` / `deleteAnnotation()`。其中 `DocInfo`/`SearchHit`/`Rect`/`AnnotationInfo`/`CreateAnnotationRequest` 的 TS 型別定義即對應上述 JSON 結構（`DocInfo.pageCount` 對應後端手動組出的 `pageCount` 欄位）。

## 相關頁面

- [Architecture.md](Architecture.md) — 座標系與渲染管線說明
- [Backend.md](Backend.md) — 各 endpoint 背後的模組與型別定義
- [Frontend.md](Frontend.md) — 前端如何呼叫這些 API

---
最後更新：2026-07-10（Phase 2）
