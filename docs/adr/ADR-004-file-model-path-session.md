# ADR-004: 檔案模型 — session 抽象保留，本機 session 由路徑建立

- **狀態**: Proposed（草案 2026-07-17）
- **決策者**: richard + 維運團隊
- **前置**: ADR-001, ADR-003

## Context

多人版語意是「網站」：multipart 上傳 → session UUID → 操作 → 下載結果。
桌面版語意是「Acrobat」：native dialog / 檔案關聯拿到**路徑** → 直接編輯 → 「儲存」= 覆寫原檔。

兩者若各長一套檔案流，`pdf-core` 以上全部分岔。分岔點必須壓到最低層。

## Decision

1. **session 抽象保留**，下沉到 `pdf-core`，來源雙態：

```
SessionSource::Upload { bytes }   // server：multipart 上傳（現狀不動）
SessionSource::Path   { path }    // desktop：native dialog / 檔案關聯給路徑
```

2. session 建立後，內部一致：同一份 UUID、快取、頁面渲染、編輯管線 — `pdf-core` 之上程式碼**看不出來源差異**
3. 桌面版 Acrobat 語意：
   - **開檔**：Tauri native dialog（或 .pdf 關聯啟動參數）→ `Path` session
   - **儲存（Ctrl+S）**：寫回原路徑。採 **write-temp-then-rename** 原子寫，防寫到一半斷電壞原檔
   - **另存新檔**：native save dialog → 新路徑
   - **dirty 追蹤**：有未存修改，關窗前必攔提示（存 / 不存 / 取消）
4. 多人版 API 與行為**零改動**：upload/download endpoint 只在 `server` crate
5. 前端 `mode=local` 時：隱藏上傳 UI，開/存走 Tauri command；`api.ts` 檔案操作抽介面，兩模式各自實作

## Consequences

- (+) `pdf-core` 單一編輯管線，PDF 正確性驗證（PDFium 重載、截圖）兩版通用
- (+) 桌面體驗真 Acrobat：無「上傳你自己的檔案」的假 web 感
- (−) `storage.rs` 需重構為 source-aware
- (−) 原子寫在 Windows（NTFS rename 語意）與 Linux 行為差異需各自測

## Alternatives rejected

- **桌面版沿用 upload 流（讀檔塞 multipart）**：儲存要「下載再覆蓋」，dirty/覆寫語意做不乾淨，體驗假
- **廢 session 抽象、桌面直接操作路徑**：pdf-core 以上出現兩套管線，維護分岔

## TODO（實作時補）

- [ ] 大檔案策略：`Path` session 是否 mmap / 串流載入（避免整檔進 RAM）
- [ ] 外部程式同時改同一檔的偵測（mtime 檢查即可？）
- [ ] 最近開啟清單（recent files）存放位置與格式
