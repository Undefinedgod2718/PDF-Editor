# ADR-005: 驗證分層與桌面發佈需求

- **狀態**: Proposed（草案 2026-07-17）
- **決策者**: richard + 維運團隊
- **前置**: ADR-002, ADR-003, ADR-004

## Context

現有驗證手冊（`.claude/skills/verification`）整套依賴 Chrome 工具鏈（claude-in-chrome 互動、DevTools performance trace）。桌面版 Linux 端是 WebKitGTK — 該工具鏈碰不到；Windows 端 WebView2 是 Chromium，可部分接回。

多人版收尾驗證**不在本 ADR 範圍**（第三方 review 進行中，流程不動）。

## Decision

### 驗證分層（三層）

| 層 | 內容 | 工具 | 覆蓋 |
| :--- | :--- | :--- | :--- |
| 1. UI 邏輯 | React 共用碼（兩版同一份） | 現有驗證手冊，Chrome 上跑 web build | ~9 成 |
| 2. PDF 正確性 | `pdf-core` 輸出 | 現有清單：PDFium 重載、截圖、Acrobat 相容 | 兩版通用 |
| 3. 桌面專屬 | 視窗、native dialog、檔案關聯、Ctrl+S/dirty、渲染差異、Excalidraw 流暢度 | 人工煙霧清單，Win + Linux 各一份 | 桌面版每 release 必跑 |

- **Windows 半自動**：WebView2 開 `--additional-browser-arguments=--remote-debugging-port=<n>` 接 CDP，可掛回既有自動化（僅開發/驗證 build，release build 關閉）
- **Linux 認命人工**：WebKitGTK 只有自家 inspector；第 3 層清單用人眼 + 截圖證據
- 證據規則沿用手冊：截圖/數據附回報，不可只說「測過了」

### 第 3 層煙霧清單骨架（首版）

- [ ] 雙擊 .pdf 由本 app 開啟（檔案關聯）
- [ ] 開多頁 + 中文 + 表單樣本 PDF，捲動不掉幀
- [ ] Excalidraw 標註：拖曳/縮放流暢（WebKitGTK 重點盯）
- [ ] Ctrl+S 覆寫原檔；PDFium 重載不壞檔
- [ ] dirty 關窗攔截三選項行為正確
- [ ] 斷電模擬（kill -9 存檔中）原檔不壞（原子寫驗證）

### 發佈需求（交團隊，開發前備妥）

| 項目 | 需求 |
| :--- | :--- |
| Windows 簽章 | code signing 憑證（無簽章 = SmartScreen 攔截） |
| Tauri updater | 簽章金鑰對；私鑰進 ops vault（沿用 `SSH Local` confidential 慣例，不落 repo） |
| WebView2 | 離線安裝器隨 installer bundle |
| Linux 打包 | .deb + AppImage；依賴 webkit2gtk-4.1（Debian 11 以前不支援，明示放棄） |
| Python sidecar | docx/xlsx 匯出依賴 Python（`sidecar.rs`）— installer 須 bundle Python 環境，或缺 sidecar 時功能偵測停用（UI 灰掉，非 500） |
| 發佈管道 | 本開發機無 GitHub push 權限 — 產物交付與簽章流程由團隊定義 |

## Consequences

- (+) 9 成驗證成本不因桌面版增加（共用層照舊）
- (−) 每次桌面 release 多一輪雙 OS 人工煙霧
- (−) 簽章/updater 金鑰管理是新維運面，需 ops vault 收編

## TODO（實作時補）

- [ ] 煙霧清單擴充成正式 checklist（隨 desktop 功能長）
- [ ] CDP 接回自動化的實作（dev build flag）
- [ ] release pipeline 定案後回填本 ADR
