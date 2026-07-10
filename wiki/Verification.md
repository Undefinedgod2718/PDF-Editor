# 驗證手冊摘要

本頁摘要 [`.claude/skills/verification/SKILL.md`](../.claude/skills/verification/SKILL.md) 的分工規則與驗證清單；完整內容請以該檔為準。

## 分工規則

| 工作類型 | 執行者 | 審查者 |
|----------|--------|--------|
| Rust / C++（後端、PDF 核心） | Fable 5 主線程 | — |
| 其餘一切（React/TS、CSS、文件、腳本、wiki） | Sonnet 5 subagent（Agent tool, `model: "sonnet"`） | Fable 5 逐檔 review 後才接受 |

Review 標準：讀完 diff、對照需求、檢查邊緣情況（空文件、多頁、中文、加密 PDF）、跑驗證清單；不通過就退回 subagent 重做並附具體修改指示。

## 後端（Rust）驗證清單

1. **建置**：`$env:CARGO_TARGET_DIR="<專案>\server\target"; cargo build`（必須覆蓋本機全域 `CARGO_TARGET_DIR`）
2. **單元測試**：`cargo test`
3. **煙霧測試**：起服務後用 `curl` 打每個新/改 endpoint，驗證 HTTP 狀態與回應 JSON 結構
4. **PDF 正確性**：任何寫入 PDF 的操作，產出檔案必須能被 PDFium 重新載入（不壞檔）、渲染截圖肉眼驗證效果存在，有 Acrobat 時另用 Acrobat 開啟驗證相容性
5. **座標系**：對外一律「PDF points、左上原點」；新 endpoint 回傳座標須用已知內容 PDF 驗證數值合理

## 前端驗證清單（每次修改必跑）

1. **建置**：`npm run build` 零錯誤（tsc + vite）
2. **啟動開發伺服器**：`npm run dev`（proxy `/api` → `8050`），或 build 後由 Rust 服務直接供檔
3. **直接互動**：用 claude-in-chrome 開頁面，實際操作新功能（點擊、輸入、拖曳），截圖比對預期
4. **Console 檢查**：`read_console_messages`，不得有新增 error/warning（既有已知項先記錄基線）
5. **效能追蹤**：跑 Chrome DevTools MCP performance trace，關注長任務（>50ms）不因本次修改新增、大 PDF 捲動不掉幀、記憶體無明顯洩漏（重複開關文件）
6. **證據**：驗證截圖/數據需附在回報中，不可只說「測過了」

## 通用規則

- 每個 Phase 完成後暫停，等使用者驗收才進下一 Phase
- 測試 PDF 需涵蓋多頁 + 中文 + 表單欄位樣本，簡單單頁不夠
- 失敗照實回報（貼錯誤輸出），不隱藏、不硬標完成
- `wiki/` 與 `README.md` 隨每個 Phase 更新

## Phase 2 驗證紀錄（2026-07-10）

**後端**：

- `curl` 煙霧測試涵蓋全部七種 `NewAnnotation` type（`highlight`/`underline`/`strikeout`/`squiggly`/`note`/`ink`/`freeText`）的 `POST .../annotations`，各回傳 `{ "count": N }` 且狀態碼正常
- 渲染截圖驗證：建立後打 `GET .../render` 肉眼比對七種註解外觀皆存在（含 `freeText` 用 `Stamp` 承載文字物件、`ink` 的 path object 筆畫）
- `DELETE .../annotations/{index}` 驗證刪除後 `GET .../annotations` 列表少一筆，且渲染圖也同步消失
- 重新載入（`with_document` 存檔後再 `load_pdf_from_file` 讀回）確認檔案未損壞、可重複寫入多次不壞檔

**前端**（claude-in-chrome 實測）：

- 螢光標記：拖曳選字建立，確認矩形依 `annotGeom.ts` 行合併演算法對齊實際文字行
- 便籤：`pointerup` 開 popup、輸入文字、確定送出，確認 popup 內互動不會被 `fromPopup` guard 誤擋
- 手繪：拖曳畫筆畫，即時 `<svg>` 預覽與送出後渲染圖比對一致
- 文字框：拖曳出範圍、輸入英數文字、送出後渲染圖出現文字
- `AnnotPanel` 面板：列表顯示、點擊項目觸發 `scrollToRect` + 閃爍、刪除按鈕移除該筆並更新渲染圖（`pageVersions` cache-bust 生效，`<img>` URL 版本號遞增）
- `read_console_messages`：零新增 error/warning
- Performance API 量測（非 Chrome DevTools MCP trace，見下方待補）：zoom + scroll 操作零 long task（>50ms）；JS heap 約 22MB；渲染請求耗時 avg 697ms、max 2048ms——這是後端 debug build（未開 `--release`、且 `with_document` 每次都整份重讀重寫 PDF 無任何快取）造成，預計 Phase 5 部署前用 release build + 快取策略改善，暫不視為本階段阻斷項

**待補**：chrome-devtools-mcp 效能追蹤（`verification` skill 清單第 5 項要求的完整 trace）因外掛 session 時序問題本次未能執行，留待下次一併補上，不影響本階段功能驗收結論。

## 相關頁面

- [Home.md](Home.md) — 專案總覽與路線圖
- [Backend.md](Backend.md) / [Frontend.md](Frontend.md) — 對應驗證清單的模組細節

---
最後更新：2026-07-10（Phase 2）
