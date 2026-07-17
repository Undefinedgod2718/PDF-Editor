# ADR-002: 桌面殼 = Tauri v2 + 系統 WebView；前端單一 React codebase

- **狀態**: Proposed（草案 2026-07-17）
- **決策者**: richard + 維運團隊
- **前置**: ADR-001
- **被引用**: ADR-003, ADR-005

## Context

單人服務的使用者故事是「Acrobat」：雙擊 .exe、.pdf 檔案關聯、原生檔案對話框、離線可用。純瀏覽器方案（`--local` flag 開瀏覽器、PWA）給不了檔案關聯與桌面整合，出局。

前端現況：React 19 + TS + Excalidraw（`web/`，~324K src），已通過多人版驗證。

## Decision

1. **桌面殼採 Tauri v2**，使用系統 WebView：
   - Windows: WebView2（Chromium 系；bundle 離線安裝器）
   - Linux: WebKitGTK（需 webkit2gtk-4.1）
2. **前端維持單一 React codebase**（`web/`），build/runtime flag `mode = web | local`：
   - `web`：現有多人版行為（upload、登入）
   - `local`：Tauri API（native dialog、路徑直開，見 ADR-004）
3. **明確不做**：
   - WASM UI 前端（Leptos/Yew/Dioxus）— 無量化收益，DOM 密集 UI 跨 wasm-bindgen 邊界更慢，Excalidraw 無替代品，且前端分岔成兩套
   - CEF / Electron — 引擎一致性換 100-200MB 體積；接受 WebKitGTK 行為差異，風險由 ADR-005 驗證分層吸收

## Consequences

- (+) binary ~10MB + pdfium 動態庫；RAM 遠低於 Electron
- (+) feature 只寫一次（React 共用），團隊只養一套 UI
- (−) Linux WebKitGTK canvas 效能與 Chrome 有差；Excalidraw 拖曳/縮放需實測（ADR-005 煙霧清單）
- (−) 放棄無 webkit2gtk-4.1 的舊發行版（Debian 11 以前）
- (−) 桌面發佈鏈需求（簽章、updater）落在團隊，見 ADR-005

## Alternatives rejected

- **PWA**：無檔案關聯、File System Access API 限 Chrome 系，不符 Acrobat 故事
- **Electron/CEF**：體積與記憶體代價，Rust 後端還得走 node 橋或 sidecar
- **WASM UI**：見上，已棄

## TODO（實作時補）

- [ ] Tauri `fileAssociations`（.pdf 關聯）+ 單一實例（第二次雙擊開新分頁或新視窗？）
- [ ] `mode` flag 注入方式（Vite define vs runtime 偵測 `window.__TAURI__`）
- [ ] WebView2 離線安裝器 bundle 設定
