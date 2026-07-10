# PDF Editor Wiki

本頁是 PDF Editor 專案的總覽與導覽首頁。

## 專案目標

PDF Editor 是一套仿 Adobe Acrobat DC 的 Web PDF 編輯器：Rust（Axum）後端負責 PDF 上傳、PDFium 渲染、文字層與搜尋 API，並直接服務前端靜態檔；React 18 + TypeScript（Vite）前端提供仿 Acrobat 的檢視器 UI。整個服務以單一連接埠對外提供（預設 `8050`），詳見 [`README.md`](../README.md)。

## 階段路線圖

| Phase | 內容 | 狀態 |
|-------|------|------|
| P1 | 檢視器：渲染、翻頁、縮放、縮圖、搜尋 | 已完成 |
| P2 | 註解：螢光標記、底線、刪除線、波浪線、便籤、手繪、文字框 | 已完成（2026-07-10） |
| P3 | 內容編輯：文字/圖片編輯、頁面增刪旋轉、合併分割 | 規劃中 |
| P4 | 表單填寫/建立、電子簽名 | 規劃中 |
| P5 | 部署至 `192.168.17.56:8050`（Windows，NSSM 服務） | 規劃中 |

每個 Phase 完成後會暫停，等待使用者驗收再進入下一階段（見 [Verification.md](Verification.md)）。

## 頁面目錄

- [Architecture.md](Architecture.md) — 系統架構：後端 worker thread 模式、前端、渲染管線、座標系
- [Backend.md](Backend.md) — Rust 後端模組導覽、環境變數、建置注意事項
- [Frontend.md](Frontend.md) — React 前端元件結構、縮放策略、開發模式 proxy
- [API.md](API.md) — 完整 API 參考（含真實 JSON 回應欄位）
- [Verification.md](Verification.md) — 分工規則與驗證清單摘要
- [Deployment.md](Deployment.md) — Phase 5 部署計畫（尚未執行）

---
最後更新：2026-07-10（Phase 2）
