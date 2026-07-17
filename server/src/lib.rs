//! axum 多人服務 lib：router 與共享狀態。
//! lib+bin 雙 target — desktop（ADR-003 Phase 1）內嵌本 lib 的 router。
//! PDF 邏輯一律在 `pdf-core`；此 crate 只有 HTTP 層與 LLM proxy。

pub mod api;
pub mod llm;

// 讓既有 `crate::pdf::…`／`crate::storage`／`crate::sidecar` 路徑照常解析。
pub use pdf_core::{pdf, sidecar, storage};

use std::sync::Arc;

pub struct AppState {
    pub storage: storage::Storage,
    pub engine: pdf::engine::PdfEngine,
}

pub type SharedState = Arc<AppState>;
