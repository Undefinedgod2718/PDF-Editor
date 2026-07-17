# 交接規格：前端 `mode=local`（桌面版檔案模型 UI）

> 對象：前端第三方開發。後端（Rust）已完成並驗證，本文件是唯一需要的介面契約。
> Review：完成後由 Fable 5 逐檔 review 才合併（`.claude/skills/verification` 分工）。
> 背景決策：`docs/adr/ADR-002`（單一 React codebase）、`ADR-004`（檔案模型）。

## 0. 大原則

- **一套 React codebase**（`web/`），不開新專案、不引入 Tauri JS API/npm 套件
- 桌面能力全部走既有 same-origin HTTP（`/api/local/*`），**認證零處理**（token cookie 由桌面殼注入，fetch 自動帶上）
- `mode=web` 行為**一個位元組都不能變** — 多人版在第三方 review 收尾，不能受影響

## 1. Mode 偵測

啟動時 `GET /api/local/ping`：

- `200 {"mode":"local"}` → 桌面模式
- 404 → web 模式（多人版 server 沒有此路由）

偵測結果放 context/store，UI 依此分流。

## 2. 桌面模式 UI 需求（Acrobat 語意）

| 項目 | 行為 |
| :--- | :--- |
| 開啟 | 按鈕/選單「開啟…」→ `POST /api/local/open-dialog`；**204 = 使用者取消，靜默返回**；200 回 DocMeta → 進既有 loadDoc 流程 |
| 隱藏 | 上傳 drop zone、上傳按鈕、文件列表的「上傳」語意，local 模式全部不顯示 |
| 儲存 Ctrl+S | `POST /api/local/documents/{id}/save`；成功 → 更新 meta；**409 = 檔案被外部程式改過** → 對話框問「仍要覆寫？」→ 是 → 帶 `{"force":true}` 重送 |
| 另存 Ctrl+Shift+S | `POST /api/local/documents/{id}/save-as-dialog`；204 = 取消；200 回新 meta（filename/origin 已變）→ 更新 UI 標題 |
| Dirty 指標 | `meta.revision !== meta.saved_revision` → 視窗標題加 `*`（如 `report.pdf* — PDF Editor`，用 `document.title`） |
| 關閉攔截 | dirty 時 `beforeunload` 攔截（WebView 支援度有限，盡力而為；原生視窗關閉攔截屬 Rust 端 Phase 2，不歸本次） |
| 下載按鈕 | local 模式改文案為「另存新檔」直接觸發 save-as-dialog（或隱藏，擇一，PR 說明理由） |

## 3. API 契約（全部同 origin、cookie 自動帶）

### `GET /api/local/ping`
`200 {"mode":"local"}`

### `POST /api/local/open-dialog`（無 body）
- `200` DocMeta（見 §4）
- `204` 使用者取消
- `422 {"error":"..."}` 選了無法開的檔

### `POST /api/local/open` body `{"path":"/abs/path.pdf"}`
同上（無 204）。前端一般用不到 — 檔案關聯/測試入口，勿在 UI 呼叫。

### `POST /api/local/documents/{id}/save` body 可省略或 `{"force":true}`
- `200` DocMeta（`saved_revision` 已對齊 `revision`，`origin_mtime` 更新）
- `409 {"error":"origin file changed on disk since open/last save"}` → force 流程
- `422` 無 origin（upload session 誤呼叫）等

### `POST /api/local/documents/{id}/save-as-dialog`（無 body）
- `200` DocMeta（`filename`/`origin` 指向新路徑）
- `204` 取消

### `POST /api/local/documents/{id}/save-as` body `{"path":"..."}`
測試入口，UI 勿用。

## 4. DocMeta 新欄位（既有欄位不變）

```json
{
  "id": "uuid", "filename": "doc.pdf", "size": 706, "revision": 1,
  "protection_hash": null,
  "origin": "/home/user/doc.pdf",     // local session 才有；web 模式永遠缺席
  "origin_mtime": 1784275057,          // 同上
  "saved_revision": 1                  // 恆在（upload session = 0）
}
```

`web/src/api.ts` 的型別補上三個 optional 欄位即可，web 模式邏輯不得讀取。

## 5. 驗證清單（PR 附證據，缺一退回）

1. `npm run build` 零錯誤（tsc + vite）
2. **web 模式迴歸**：Chrome 開多人版（proxy /api → 8050），上傳/編輯/下載照舊，console 無新 error — 截圖
3. **local 模式**：`cargo build -p pdf-editor-desktop` 後由後端團隊起桌面 app 驗（或用 `PDF_EDITOR_TOKEN` + curl 模擬回應照 §3 契約寫 mock 測）：開啟→編輯→Ctrl+S→標題 `*` 消失；409→force 流程 — 截圖
4. 中文檔名 PDF 開啟/儲存正常
5. 不新增 npm dependency；`git diff --stat` 附在 PR

## 6. 後端現狀（參考，勿改）

- 分支 `refactor/adr-001-workspace`；desktop crate = Tauri v2 殼 + 內嵌 axum（隨機埠 + token cookie）
- 後端煙霧已驗：open → rotate → save 原子寫回 → PDFium 重載 → 渲染正確；409/force/save-as 全通過
- 你的開發環境：web 模式照舊 `npm run dev`（proxy 8050）開發即可，local 分流邏輯用 §3 契約 mock
