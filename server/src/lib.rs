//! axum 多人服務 lib：router 與共享狀態。
//! lib+bin 雙 target — desktop（ADR-003 Phase 1）內嵌本 lib 的 router。
//! PDF 邏輯一律在 `pdf-core`；此 crate 只有 HTTP 層與 LLM proxy。

pub mod actions;
pub mod api;
pub mod llm;

// 讓既有 `crate::pdf::…`／`crate::storage`／`crate::sidecar` 路徑照常解析。
pub use pdf_core::{pdf, sidecar, storage};

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

pub struct AppState {
    pub storage: storage::Storage,
    pub engine: pdf::engine::PdfEngine,
    /// Background OCR jobs (`POST .../ocr` returns a job id immediately;
    /// `GET .../ocr/jobs/{job_id}` polls this map for progress/result).
    /// Locked only for quick map ops — never held across an `.await`.
    pub ocr_jobs: Mutex<HashMap<uuid::Uuid, api::OcrJobState>>,
    /// P17 動作精靈：saved pipelines (persisted) + in-flight/finished batch
    /// runs (in-memory only, bounded — see `actions::ActionRuns`).
    pub actions: actions::ActionStore,
    pub action_runs: Mutex<actions::ActionRuns>,
}

pub type SharedState = Arc<AppState>;
