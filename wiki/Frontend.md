# 前端（web/）

本頁說明 `web/` 的元件結構與資料流、縮放策略，以及開發模式的 proxy 設定。

## 元件結構與資料流

前端為 React 18 + TypeScript（Vite），狀態集中在 `web/src/App.tsx`，透過 props 往下傳給各元件；沒有全域狀態管理套件。

```
App.tsx（狀態擁有者：doc / scale / currentPage / showThumbs / showSearch / hits / activeHit
          / tool / color / inkWidth / showAnnotPanel / pageVersions / flash）
 ├─ Toolbar.tsx        開檔、下載、翻頁、縮放、切換縮圖／搜尋面板
 ├─ ThumbnailPanel.tsx 頁面縮圖列表，點擊跳頁
 ├─ AnnotToolbar.tsx   工具選擇、色盤、手繪筆寬、開關 AnnotPanel（Phase 2）
 ├─ Viewer.tsx          主檢視區：渲染各頁 <img>、捲動偵測目前頁、疊加搜尋高亮
 │   └─ AnnotLayer.tsx（每頁一份）pointer overlay：拖曳建立標記/手繪/文字框、便籤 popup、選取閃爍（Phase 2）
 ├─ SearchPanel.tsx     搜尋輸入、結果清單、上一筆/下一筆導覽
 └─ AnnotPanel.tsx      目前頁註解列表、刪除、點擊跳轉並閃爍（Phase 2）
```

- **App.tsx**：`openFile()` 呼叫 `uploadPdf()` 上傳、`fetchDocInfo()` 取得文件資訊後存入 `doc` state；用 `viewerRef`（`useImperativeHandle`）取得 `Viewer` 暴露的 `scrollToPage()` / `scrollToRect()`，供 `Toolbar` 翻頁、`SearchPanel` 跳到命中頁、`AnnotPanel` 跳到選取的註解使用；`Ctrl/Cmd+F` 全域快捷鍵開啟搜尋面板，`Escape` 把註解工具切回 `select`。
- **Toolbar.tsx**：開檔（`<input type="file">`）、下載連結（`downloadUrl(doc.id)`）、縮圖／搜尋面板顯示切換、頁碼輸入框、以固定階梯 `ZOOM_STEPS = [0.5, 0.75, 1, 1.25, 1.5, 2, 3, 4]` 逐級縮放。
- **ThumbnailPanel.tsx**：以固定 `THUMB_SCALE = 0.25` 呼叫 `renderUrl()` 取得每頁縮圖 PNG，`loading="lazy"`，點擊呼叫 `gotoPage()`。
- **Viewer.tsx**：`forwardRef` 暴露 `scrollToPage(p)` 與 `scrollToRect(p, rect)`（Phase 2 新增，供 `AnnotPanel` 選取跳轉使用，捲到目標矩形上方留 96px 邊距）；用 `<div className="page">` 依 `page.width/height * scale` 設定 CSS 尺寸，內含渲染圖、搜尋命中高亮 `<div className="hl">`（位置 = 命中矩形座標 × `scale`）與每頁一份的 `AnnotLayer`；監聽容器 `scroll` 事件，以「視窗中線落在哪一頁」判斷 `currentPage` 並回呼 `onCurrentPageChange`。
- **SearchPanel.tsx**：輸入框 Enter 或按鈕觸發 `searchDoc()`，結果存回 `App` 的 `hits`；上一筆/下一筆循環導覽（`gotoHit`），點結果項也可直接跳頁。

## 註解 UI 架構（Phase 2）

### `AnnotToolbar.tsx`

八個工具按鈕（`select`/`highlight`/`underline`/`strikeout`/`squiggly`/`note`/`ink`/`freeText`，`AnnotTool` 型別）、8 色固定色盤 + `<input type="color">` 自訂色、手繪筆寬（`ink` 工具啟用時才顯示，`INK_WIDTHS = [1, 2, 4, 8]`）、`freeText` 工具啟用時顯示「僅支援英數」提示（對應後端只內建 Helvetica、不支援中文的限制），以及開關 `AnnotPanel` 的按鈕。所有狀態由 `App.tsx` 擁有，這裡純受控元件。

### `AnnotLayer.tsx`

每頁疊一份的 pointer-event overlay（`annot-layer`，`tool !== 'select'` 時加 `annot-layer-active` 讓 CSS 開啟 pointer 攔截）。四種互動模式依 `tool` 分派：

- **文字標記**（`highlight`/`underline`/`strikeout`/`squiggly`）：拖曳出選取矩形（px），放開時换算成 PDF points，呼叫 `getPageChars()`（`Viewer.tsx` 內以 `Map` 快取每頁 `fetchPageText()` 結果，避免重複打 API）取得該頁字元框，再用 `annotGeom.ts::selectionToLineRects()` 算出實際要送出的矩形，送 `createAnnotation()`。`highlight` 預設色會強制 alpha `150`（半透明，避免蓋住文字），其餘標記沿用選色。
- **手繪**（`ink`）：pointer down/move 累積點陣列，即時畫 `<svg><polyline>` 預覽；放開時整條路徑（≥2 點）換算成 points 送出（單次送出目前固定只含一條 stroke）。
- **文字框**（`freeText`）：拖曳出矩形，放開後在矩形下方開 popup（textarea + 字級下拉 12/14/18/24pt），確定才送出 `createAnnotation`。
- **便籤**（`note`）：popup 故意開在 **`pointerup`、不是 `pointerdown`**——如果在 pointerdown 開啟，接下來的 mouseup 會落在剛出現的 overlay/popup 上而不是新 focus 的 textarea，把 `autoFocus` 搶走的焦點打掉，導致打字打到 `<body>`。

所有互動送出失敗（`tryCreate()` 包裝）只印 `console.error`，不丟例外中斷（Phase 2 尚無全域 toast 元件）。

**`fromPopup` guard**：便籤與文字框的 popup（`.annot-popup`）內部元素（textarea、確定/取消按鈕）的 pointer 事件會冒泡回外層 overlay 的 `onPointerDown/Move/Up`。若不擋掉，overlay 會誤判成「在 popup 位置又開始一次新的拖曳/繪製」，清空正在輸入的 `noteText`/`freeTextValue`，或用 pointer capture 劫持按鈕的 click。`fromPopup(e)` 用 `(e.target as HTMLElement).closest('.annot-popup')` 偵測並提早 return 解決。

建立/刪除成功後透過 `onChanged()` 回呼 `App.tsx` 的 `bumpPageVersion(page)`（見下）；`flashRect`/`flashKey` 兩個 prop 由 `AnnotPanel` 點擊項目觸發，畫一個短暫高亮框（`annot-flash`，`key={flashKey}` 讓 React 每次都重新掛載觸發 CSS 動畫）。

### `AnnotPanel.tsx`

目前頁註解列表：`useEffect` 依 `[doc.id, currentPage, version]` 變化重新 `listAnnotations()`（`version` 即 `pageVersions[currentPage]`，見下方 cache-bust 機制，用來在同頁建立/刪除後觸發重新整理）。每筆顯示 `TYPE_LABEL` 對照後的中文類型名稱（**後端回傳的是 PDFium subtype 大寫命名**，如 `Stamp` 對應顯示「文字框」，因為文字框後端實際是用 `Stamp` annotation 儲存，見 [API.md](API.md#註解-endpoint-phase-2)）、備註內容、矩形座標；點項目呼叫 `onSelect(page, rect)` 觸發 `App.tsx` 的 `scrollToRect` + `flash`；刪除按鈕呼叫 `deleteAnnotation()` 後回呼 `onDeleted(page)`（同樣是 `bumpPageVersion`）。

### `annotGeom.ts`

- `rectsIntersect(a, b)`：兩矩形（同座標系）是否相交，AABB 判定。
- `selectionToLineRects(chars, selection)`：把使用者的拖曳選取矩形轉成「每行一個矩形」的行合併演算法——先用 `rectsIntersect` 篩出與選取矩形相交的字元，依 `(y, x)` 排序，再逐字元找「同行」（判定條件：字元中心 y 與該行第一個字元中心 y 的差 < 兩者字高較大值的一半），最後每行取所有字元的 bounding box。這一步是為了讓拖曳跨行選取時，產生的 highlight/underline 矩形跟畫面上實際的文字行對齊，而不是單一個涵蓋多行的大矩形。

### `pageVersions` cache-bust 機制

註解建立/刪除是後端直接把新內容烙進 PDF 檔案本身（見 [Backend.md](Backend.md#serversrcpdfannotsrsphase-2) 的 `with_document` 原子存檔），但頁面渲染圖走 `<img src="/api/.../render?scale=...">`，瀏覽器/CDN 可能快取同一個 URL。`App.tsx` 用 `pageVersions: Record<number, number>` 記錄每頁的版本號，`bumpPageVersion(page)` 在每次 `AnnotLayer.onChanged` 或 `AnnotPanel.onDeleted` 觸發時 `+1`；`renderUrl(doc.id, page.index, scale * dpr, pageVersions[page.index])` 把版本號當成查詢字串的一部分，版本變了 URL 就變，逼瀏覽器重新請求渲染圖而不是吃快取。開新檔（`openFile`）時整份 `pageVersions` 重置為 `{}`。

## 縮放策略

顯示尺寸與請求渲染解析度是分開計算的：

- CSS 顯示尺寸 = `page.width（pt）× scale`（`scale` 為使用者透過 Toolbar 選擇的縮放層級，如 100% = `1.0`）。
- 實際請求後端渲染的解析度 = `scale × devicePixelRatio`（`devicePixelRatio` 上限 clamp 到 `2`，見 `Viewer.tsx` 的 `const dpr = Math.min(window.devicePixelRatio || 1, 2)`），確保高 DPI 螢幕下畫面不糊，同時避免螢幕 DPR 過高時請求過大的圖片。
- 縮圖固定使用低倍率 `THUMB_SCALE = 0.25`（未乘 DPR），避免縮圖流量與記憶體開銷過大。

## 開發模式 proxy 設定

`web/vite.config.ts`：

```ts
server: {
  port: 5173,
  proxy: {
    '/api': 'http://127.0.0.1:8050',
  },
}
```

開發時 `npm run dev` 啟動 Vite dev server（`:5173`），所有 `/api/*` 請求會被轉發到本機 `127.0.0.1:8050` 的 Rust 後端；正式建置後（`npm run build` 產出 `web/dist`）則由 Rust 伺服器直接服務靜態檔，不再需要 proxy（見 [Architecture.md](Architecture.md#單埠部署模式)）。

## 相關頁面

- [Architecture.md](Architecture.md) — 渲染管線與座標系
- [API.md](API.md) — 前端呼叫的完整 API 參考（`web/src/api.ts`）
- [Verification.md](Verification.md) — 前端驗證清單

---
最後更新：2026-07-10（Phase 2）
