# ADR 索引 — 單人服務（桌面版）架構

背景：專案分「多人服務」（axum，sgsac001 `192.168.17.56:8050`，收尾中、第三方 review）與「單人服務」（Acrobat 式桌面應用，本系列標的）。部署模型不變，兩者並存。

| ADR | 決策 | 狀態 |
| :--- | :--- | :--- |
| [ADR-001](ADR-001-workspace-split-pdf-core.md) | Workspace 拆分：`pdf-core` lib + `server` bin + `desktop` bin | Proposed |
| [ADR-002](ADR-002-desktop-shell-tauri-system-webview.md) | 桌面殼 Tauri v2 + 系統 WebView；單一 React 前端；不做 WASM UI / Electron / PWA | Proposed |
| [ADR-003](ADR-003-local-transport-embedded-axum-token.md) | 本機傳輸：Phase 1 內嵌 axum `127.0.0.1:0` + 一次性 token；終態 Tauri IPC | Proposed |
| [ADR-004](ADR-004-file-model-path-session.md) | 檔案模型：session 抽象保留，`Upload`/`Path` 雙來源；桌面 Ctrl+S 原子覆寫 | Proposed |
| [ADR-005](ADR-005-verification-and-release.md) | 驗證三層分工 + 桌面發佈需求（簽章/updater/打包） | Proposed |

實施順序：多人版收尾（不動）→ ADR-001 重構 → ADR-002/003/004 桌面版 → ADR-005 隨 release 落地。

目標機事實（來源 `SSH Local` 戶口名簿，2026-07-17）：

- 多人版主機 sgsac001：i5-7500 4C / 8GB RAM — 資源緊，桌面版分流重渲染負載是額外收益
- Linux 桌面目標：sgsaa002（Ubuntu 26.04 Desktop, RTX 5070 Ti）；Windows 目標含 `.53`
- sgsac001 埠 8080-8094/2222/9090/21115-21119 已佔用 — 支持 ADR-003 隨機埠決策
