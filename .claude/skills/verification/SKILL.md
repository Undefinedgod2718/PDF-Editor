---
name: verification
description: PDF Editor 專案的分工與「怎麼確認做對了」驗證手冊。每個 Phase、每次修改都必須走完對應清單才算完成。
---

# PDF Editor 驗證手冊（怎麼確認做對了）

## 分工規則

| 工作類型 | 執行者 | 審查者 |
|----------|--------|--------|
| Rust / C++（後端、PDF 核心） | Fable 5 主線程 | — |
| 其餘一切（React/TS、CSS、文件、腳本、wiki） | Sonnet 5 subagent（Agent tool, `model: "sonnet"`） | Fable 5 逐檔 review 後才接受 |

Review 標準：讀完 diff、對照需求、檢查邊緣情況（空文件、多頁、中文、加密 PDF）、跑驗證清單。不通過 → 退回 subagent 重做，附具體修改指示。

## 後端（Rust）驗證清單

1. **建置**：`$env:CARGO_TARGET_DIR="<專案>\server\target"; cargo build`（本機有全域 CARGO_TARGET_DIR 污染，必須覆蓋）
2. **單元測試**：`cargo test`
3. **煙霧測試**：起服務 → `curl` 打每個新/改 endpoint，驗 HTTP 狀態 + 回應 JSON 結構
4. **PDF 正確性**：任何寫入 PDF 的操作，產出檔案必須
   - 能被 PDFium 重新載入（不壞檔）
   - 渲染後截圖肉眼驗證效果存在
   - 有 Acrobat 時用 Acrobat 開啟驗證相容性
5. **座標系**：API 對外一律「PDF points、左上原點」。新 endpoint 回傳座標必須用已知內容 PDF 驗證數值合理

## 前端驗證清單（每次修改必跑）

1. **建置**：`npm run build` 零錯誤（tsc + vite）
2. **啟動開發伺服器**：`npm run dev`（proxy /api → 8050），或 build 後由 Rust 服務直接供檔
3. **直接互動**：用 claude-in-chrome 開頁面，實際操作新功能（點擊、輸入、拖曳），截圖比對預期
4. **Console 檢查**：`read_console_messages`，不得有新增 error/warning（既有已知項先記錄基線）
5. **效能追蹤**：跑 Chrome DevTools MCP（`chrome-devtools-mcp@claude-plugins-official` 插件）performance trace，關注：
   - 長任務（>50ms）不因本次修改新增
   - 渲染大 PDF 時捲動不掉幀
   - 記憶體無明顯洩漏（重複開關文件）
6. **證據**：驗證截圖/數據附在回報中，不可只說「測過了」

## 通用規則

- 每個 Phase 完成 → 暫停，等使用者驗收後才進下一 Phase
- 測試 PDF：簡單單頁不夠，需多頁 + 中文 + 表單欄位樣本
- 失敗照實回報（貼錯誤輸出），不隱藏、不硬標完成
- wiki（`wiki/`）與 README 隨每 Phase 更新
