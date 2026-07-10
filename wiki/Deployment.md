# 部署計畫（Phase 5）

本頁記錄 Phase 5 的部署目標與計畫，目前僅為規劃狀態，尚未執行。

## 狀態

**尚未執行。** 依 `README.md` 的開發階段規劃，部署屬於 Phase 5，需等 Phase 2–4（註解、內容編輯、表單/簽名）完成並驗收後才會進行。

## 部署目標

| 項目 | 內容 |
|------|------|
| 目標主機 | `tctest001`（`192.168.17.56`） |
| 連線方式 | `ssh user@192.168.17.56` |
| 服務埠 | `8050`（與開發時預設埠一致，見 [Backend.md](Backend.md#環境變數)） |
| 作業系統 | Windows |
| 服務管理 | 計畫用 [NSSM](https://nssm.cc/)（Non-Sucking Service Manager）把 `pdf-editor-server.exe` 註冊為 Windows 服務 |

## 計畫中的部署方式（尚未驗證）

1. `cd web && npm run build` 產出 `web/dist`
2. `cd server && cargo build --release`（注意需先覆蓋 `CARGO_TARGET_DIR`，見 [Backend.md](Backend.md#建置注意事項)）
3. 將 release 執行檔、`pdfium.dll`、`web/dist` 部署到目標主機，維持「執行檔工作目錄含 `pdfium.dll`，且 `../web/dist` 存在（或設定 `PDF_EDITOR_WEB`）」的相對關係
4. 用 NSSM 將執行檔註冊為 Windows 服務，監聽 `0.0.0.0:8050`（或以 `PDF_EDITOR_PORT` 指定）
5. 確認防火牆規則開放 `8050` 埠，讓區網內其他主機可透過 `http://192.168.17.56:8050` 存取

以上步驟為計畫內容，實際部署細節（NSSM 設定參數、資料目錄位置、更新/回滾流程）需在 Phase 5 執行時依現場狀況確認並回填本頁。

## 相關頁面

- [Home.md](Home.md) — 專案總覽與階段路線圖
- [Backend.md](Backend.md) — 環境變數、建置注意事項
- [Verification.md](Verification.md) — 每個 Phase 完成後的驗收流程

---
最後更新：2026-07-10（Phase 1）
