# ADR-003: 本機傳輸層 — Phase 1 內嵌 axum + loopback token，終態 Tauri IPC

- **狀態**: Accepted（2026-07-17 使用者核准動工）
- **決策者**: richard + 維運團隊
- **前置**: ADR-001, ADR-002
- **被引用**: ADR-004

## Context

前端一套共用（ADR-002），現有 API 是 HTTP（axum, 30+ endpoint，multipart 上傳、圖片/串流回傳）。桌面版若立刻改 Tauri IPC，所有 endpoint 與前端 `api.ts` 都要重寫，違反「最快拿到能跑的本機版」。

環境事實（來源 `SSH Local` 拓撲）：

- 多人服務固定於 sgsac001 `192.168.17.56:8050`；桌面版跑在使用者機（sgsaa002 Desktop、`.53` Windows 等），兩者互不相干 — **桌面版不連遠端 server，全部本機**
- sgsac001 上 8080-8094 埠已被 NocoDB/Wiki.js/Excalidraw/Gitea 等佔用 — 桌面版即使裝在同類主機，固定埠必撞，隨機埠迴避
- 密文管理慣例：ops vault（`wiki/confidential/` + read gate + access log），**密文不落文件、不落 repo**

## Decision

**Phase 1（出貨版）— 內嵌 axum：**

1. desktop 啟動時在同 process 起 axum，bind `127.0.0.1:0`（OS 配隨機埠）
2. 每次啟動產生一次性隨機 token（≥128-bit，CSPRNG）：
   - 只存在記憶體，**不落磁碟、不落 log**（沿用 ops vault 精神）
   - WebView 載入時由 Tauri 注入前端；此後每個 API request 帶 `Authorization: Bearer <token>`
   - axum middleware 驗 token，缺/錯一律 401 — 防同機其他程序打 loopback API
3. `local` 模式關閉多人版專屬層：argon2 登入、多 session 隔離語意照 ADR-004 調整
4. CORS 收死：只允許 Tauri WebView origin（`tauri://localhost` / `http://tauri.localhost`）

**Phase 2（終態，團隊排程）— Tauri IPC / custom protocol：**

- endpoint 逐批遷 `invoke` command 或 custom protocol，binary 回傳走 `tauri::ipc::Response`
- 全遷完關掉 loopback port
- 遷移順序建議：低頻管理類先行，渲染/串流類最後（收益驗證後才動）

## Consequences

- (+) Phase 1 前端零改動（只加 token header），最快出貨
- (+) token + loopback + CORS 三層，威脅模型（同機惡意程序）已覆蓋
- (−) Phase 1 期間本機開著一個 port（雖 loopback-only）
- (−) Phase 2 是第二次工程，ADR 在此釘死方向避免爛尾或重議

## Alternatives rejected

- **固定埠（如 8050）**：與多人版/同機服務撞埠；且可預測埠降低 token 防線價值
- **Unix socket / named pipe**：WebView fetch 不能直打，還是要橋接層，複雜度不划算
- **直接 Phase 2（全 IPC）**：重寫 30+ endpoint + multipart/串流語意，延遲出貨數週

## TODO（實作時補）

- [ ] Phase 1 實作路徑（2026-07-17 盤點定案）：`server` 拆 lib+bin，desktop 依賴 server-lib 直接掛 `api::router()`；router 需新增組態（token middleware on、argon2 登入 off、CORS 收 Tauri origin、static file serve off）
- [ ] token 注入機制（`initialization_scripts` vs window global）
- [ ] axum graceful shutdown 掛 Tauri app exit
- [ ] Phase 2 endpoint 遷移清單與順序
