//! PDF 領域核心：PDFium 渲染、lopdf 寫入、字型、表單、比對、壓縮。
//! 無 HTTP／web framework 依賴 — 供 `server`（axum 多人服務）與
//! `desktop`（Tauri 單人服務，ADR-002）共用。見 docs/adr/ADR-001。

pub mod pdf;
pub mod sidecar;
pub mod storage;
