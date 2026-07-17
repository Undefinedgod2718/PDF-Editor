# ADR-001: Workspace 拆分 — 抽出 `pdf-core` 共用核心

- **狀態**: Accepted（2026-07-17 使用者核准動工）
- **決策者**: richard + 維運團隊
- **前置**: 無
- **被引用**: ADR-002, ADR-003, ADR-004

## Context

專案要同時支撐兩種服務型態：

| 型態 | 部署 | 現況 |
| :--- | :--- | :--- |
| 多人服務 | axum server，sgsac001 `192.168.17.56:8050`（systemd / NSSM） | 開發完成，第三方 review 收尾中，**不動** |
| 單人服務 | 桌面應用（Acrobat 式體驗：雙擊開檔、檔案關聯、離線） | 本 ADR 系列的標的 |

PDF 邏輯（PDFium 渲染、lopdf 寫入、字型子集、diff、表單）目前全部長在 `server/src/pdf` 內，與 axum handler 耦合。兩種服務共用這份邏輯，不拆則桌面版被迫依賴 web framework。

## Decision

Cargo workspace 三 crate：

```
workspace
 ├─ pdf-core   # lib：PDFium、lopdf、ttf-parser、subsetter、similar
 │             # 無 axum/tokio-web 依賴；session 抽象在此層（見 ADR-004）
 ├─ server     # bin：axum 多人服務（現有 server/ 改掛 pdf-core）
 └─ desktop    # bin：Tauri v2 殼（見 ADR-002）
```

拆分原則：

- `pdf-core` 只依賴 PDF/字型/影像 crate，禁止 HTTP、認證（argon2）、檔案上傳語意
- `server` 保留：axum、multipart、argon2、LLM proxy（reqwest）
- 拆分屬純重構：`cargo test` 全綠 + 多人版煙霧測試不變，才算完成

## Consequences

- (+) 桌面版與伺服器版共用同一份 PDF 正確性保證與測試
- (+) 團隊交接邊界清楚：pdf-core = 領域邏輯；server/desktop = 傳輸殼
- (−) 一次性重構成本；期間凍結 feature
- (−) `CARGO_TARGET_DIR` 覆蓋慣例需沿用到 workspace（本機全域污染問題）

## Alternatives rejected

- **desktop 直接依賴 server crate**：拖進 axum/argon2 整包，binary 肥、邊界爛
- **複製 PDF 邏輯到 desktop**：雙份維護，必然發散

## 搬遷清單（2026-07-17 盤點；耦合實測：`pdf/*`、`storage.rs`、`sidecar.rs`、`llm.rs` 零 axum 依賴）

| 模組 | 行數 | 去向 | 備註 |
| :--- | ---: | :--- | :--- |
| `pdf/`（engine, ops, objects, imageops, font, protect, textedit, formops, exportops, compare, annots, pageops, compress, formbuild） | 5,964 | `pdf-core` | 原封搬移，無需解耦 |
| `storage.rs` | 190 | `pdf-core` | session/DocMeta 層；ADR-004 `SessionSource` 改造點在此 |
| `sidecar.rs` | 193 | `pdf-core` | shell out Python（docx/xlsx 匯出）；**桌面打包影響見下** |
| `llm.rs` | 120 | `server`（lib） | reqwest 網路功能；桌面版經 server-lib 取得，離線 degrade |
| `api/mod.rs` | 1,530 | `server`（lib） | `api::router()` 導出為 lib API，加 auth/token 組態 |
| `main.rs` | 70 | `server`（bin） | `AppState`/`SharedState` 下沉 lib |

**修正**：`server` 改為 **lib + bin** 雙 target。Phase 1 desktop 依賴 `server` lib 取得現成 router（ADR-003 內嵌 axum 的實作路徑）；Phase 2 IPC 遷移完成後解除此依賴。原「desktop 不依賴 server」原則改為**終態原則**，過渡期明示允許。

**桌面打包影響（回填 ADR-005）**：docx/xlsx 匯出依賴 Python sidecar — 桌面版須 bundle Python 環境（Tauri sidecar/resource），或該功能偵測缺 sidecar 時優雅停用。

## TODO（實作時補）

- [ ] 拆分後 crate 版本策略（workspace version 統一 vs 各自）
- [ ] `AppState` 拆分：`Storage`+`PdfEngine` 歸 pdf-core 組合型，router 組態歸 server
