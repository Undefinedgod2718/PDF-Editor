//! P17 動作精靈（Action Wizard）：使用者存的操作序列（pipeline），可套一或
//! 多份既有文件批次執行。
//!
//! Step 的可持久化欄位刻意排除密碼（`Protect`/`Encrypt` 的
//! owner/user password）：action 定義存在 `{data}/actions/{id}.json`，
//! 這條路徑後面沒有任何身分驗證（本 app 目前整體無登入）——存密碼進這份
//! 檔案等於讓 LAN 上任何打得到 API 的人都能無限次讀回明文密碼，比起現有
//! 單次請求 body 帶密碼、用完即忘的作法（`/protect`、`/encrypt`）風險高
//! 得多。密碼只能在 `run` 當下的 request body（[`StepSecrets`]）帶入，
//! 從不落地。
//!
//! 每個 step 的執行結果是「同一份文件（原地變更，bump revision）」或
//! 「新文件（`Storage::save` 產生新 id）」，跟現有單發 API（compress/
//! protect/encrypt/ocr/redact）一致；只有 `Export` 是終端 —— 之後不能再接
//! 別的 step（存 action 時驗證），輸出的是檔案 bytes 而非 PDF 文件。

use std::collections::{HashMap, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, RwLock};

use serde::{Deserialize, Serialize};
use uuid::Uuid;

use pdf_core::pdf::{compress, exportops, ocr, pageops, protect, redact};
use pdf_core::{sidecar, storage};

use crate::api::{CompressPreset, ExportFormat as ApiExportFormat};
use crate::SharedState;

// ---------------------------------------------------------------------
// Definition
// ---------------------------------------------------------------------

#[derive(Clone, Serialize, Deserialize)]
pub struct StepRedactBox {
    pub page: u16,
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
}

/// One operation in a saved pipeline. Field shapes mirror the equivalent
/// single-shot endpoint's request body so the frontend can reuse the same
/// param forms/validation it already has for each tool.
#[derive(Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "camelCase")]
pub enum Step {
    RotateAll {
        delta: i32,
    },
    Crop {
        pages: Vec<u16>,
        rect: Option<pageops::CropRect>,
    },
    Resize {
        pages: Vec<u16>,
        width: f32,
        height: f32,
        mode: pageops::ResizeMode,
    },
    Compress {
        preset: CompressPreset,
        /// Custom preset only; ignored otherwise.
        dpi: Option<f32>,
        quality: Option<u8>,
    },
    /// Owner-password permission lock. Password supplied at run time via
    /// [`StepSecrets::owner_password`] — never stored in the action.
    Protect {
        permissions: protect::PermissionFlags,
    },
    /// Open-password encryption. Passwords supplied at run time via
    /// [`StepSecrets`] — never stored in the action.
    Encrypt {
        permissions: Option<protect::PermissionFlags>,
    },
    Ocr {
        langs: Option<String>,
        dpi: Option<f32>,
        min_confidence: Option<f32>,
        #[serde(default)]
        force: bool,
    },
    /// Matches the single-shot `/redact` endpoint's own behavior: once
    /// redaction succeeds, the document being redacted is deleted from
    /// storage (replaced by the new redacted document). As the first step
    /// of a batch action this deletes *every* original in `document_ids`
    /// the moment its own redaction succeeds — same rule as always, just
    /// applied once per file instead of once per manual call.
    Redact {
        boxes: Vec<StepRedactBox>,
        dpi: Option<f32>,
        jpeg_quality: Option<u8>,
    },
    /// Terminal step: produces downloadable bytes, not a PDF document.
    /// Must be the last step in an action (validated on save).
    ///
    /// Always exports every page — unlike the single-shot `/export`
    /// endpoint, there's no per-step page filter (no `pages` field here).
    /// Add one if a pipeline ever needs partial-document export.
    Export {
        format: ApiExportFormat,
        dpi: Option<u32>,
        quality: Option<u8>,
    },
}

impl Step {
    fn is_export(&self) -> bool {
        matches!(self, Step::Export { .. })
    }

    /// Steps whose secret must be supplied fresh in `RunActionBody::step_secrets`.
    fn needs_secret(&self) -> bool {
        matches!(self, Step::Protect { .. } | Step::Encrypt { .. })
    }
}

#[derive(Clone, Serialize, Deserialize)]
pub struct ActionDef {
    pub id: Uuid,
    pub name: String,
    pub steps: Vec<Step>,
}

/// `POST .../run` request-only secrets, keyed by step index (as a string —
/// JSON object keys are always strings). Never persisted; lives only in
/// this request's memory for the duration of the run.
#[derive(Clone, Default, Deserialize)]
pub struct StepSecrets {
    pub owner_password: Option<String>,
    pub user_password: Option<String>,
}

/// Reject obviously-broken definitions before they're persisted: an empty
/// pipeline, or an `Export` step anywhere but last (its output isn't a PDF,
/// so nothing downstream could act on it).
pub fn validate_steps(steps: &[Step]) -> Result<(), String> {
    if steps.is_empty() {
        return Err("action needs at least one step".into());
    }
    for (i, step) in steps.iter().enumerate() {
        if step.is_export() && i != steps.len() - 1 {
            return Err(format!("export step at index {i} must be the last step"));
        }
    }
    Ok(())
}

/// Every step needing a run-time secret must have one in `step_secrets`,
/// checked up front so a batch of 20 files doesn't run 19 of them only to
/// fail step 3 on file 20 for a password that was never going to be there.
pub fn validate_secrets(steps: &[Step], step_secrets: &HashMap<usize, StepSecrets>) -> Result<(), String> {
    for (i, step) in steps.iter().enumerate() {
        if !step.needs_secret() {
            continue;
        }
        let secret = step_secrets.get(&i);
        match step {
            Step::Protect { .. } => {
                if secret.and_then(|s| s.owner_password.as_ref()).is_none() {
                    return Err(format!("step {i} (protect) needs owner_password in step_secrets"));
                }
            }
            Step::Encrypt { .. } => {
                if secret.and_then(|s| s.user_password.as_ref()).is_none() {
                    return Err(format!("step {i} (encrypt) needs user_password in step_secrets"));
                }
            }
            _ => unreachable!("needs_secret() only matches Protect/Encrypt"),
        }
    }
    Ok(())
}

// ---------------------------------------------------------------------
// Storage
// ---------------------------------------------------------------------

pub struct ActionStore {
    root: PathBuf,
    actions: RwLock<HashMap<Uuid, ActionDef>>,
}

impl ActionStore {
    pub fn new(root: impl AsRef<Path>) -> anyhow::Result<Self> {
        let root = root.as_ref().to_path_buf();
        std::fs::create_dir_all(&root)?;
        let mut actions = HashMap::new();
        for entry in std::fs::read_dir(&root)? {
            let path = entry?.path();
            if path.extension().is_some_and(|e| e == "json") {
                if let Ok(text) = std::fs::read_to_string(&path) {
                    if let Ok(def) = serde_json::from_str::<ActionDef>(&text) {
                        actions.insert(def.id, def);
                    }
                }
            }
        }
        Ok(Self { root, actions: RwLock::new(actions) })
    }

    fn path(&self, id: Uuid) -> PathBuf {
        self.root.join(format!("{id}.json"))
    }

    pub fn create(&self, name: String, steps: Vec<Step>) -> Result<ActionDef, String> {
        validate_steps(&steps)?;
        let def = ActionDef { id: Uuid::new_v4(), name, steps };
        std::fs::write(self.path(def.id), serde_json::to_string_pretty(&def).map_err(|e| e.to_string())?)
            .map_err(|e| e.to_string())?;
        self.actions.write().unwrap().insert(def.id, def.clone());
        Ok(def)
    }

    pub fn get(&self, id: Uuid) -> Option<ActionDef> {
        self.actions.read().unwrap().get(&id).cloned()
    }

    pub fn list(&self) -> Vec<ActionDef> {
        self.actions.read().unwrap().values().cloned().collect()
    }

    pub fn delete(&self, id: Uuid) -> Result<(), String> {
        self.actions.write().unwrap().remove(&id).ok_or("action not found")?;
        let _ = std::fs::remove_file(self.path(id));
        Ok(())
    }
}

// ---------------------------------------------------------------------
// Run: progress + job state
// ---------------------------------------------------------------------

/// Read from the polling handler (tokio side) while `run_batch` writes from
/// its own spawned task — plain atomics, no lock needed for "how far along".
#[derive(Debug, Default)]
pub struct ActionRunProgress {
    current_file: AtomicUsize,
    total_files: AtomicUsize,
    current_step: AtomicUsize,
    total_steps: AtomicUsize,
}

impl ActionRunProgress {
    pub fn new(total_files: usize, total_steps: usize) -> Self {
        Self {
            current_file: AtomicUsize::new(0),
            total_files: AtomicUsize::new(total_files),
            current_step: AtomicUsize::new(0),
            total_steps: AtomicUsize::new(total_steps),
        }
    }

    pub fn snapshot(&self) -> (usize, usize, usize, usize) {
        (
            self.current_file.load(Ordering::Relaxed),
            self.total_files.load(Ordering::Relaxed),
            self.current_step.load(Ordering::Relaxed),
            self.total_steps.load(Ordering::Relaxed),
        )
    }
}

pub enum ActionRunState {
    Running(Arc<ActionRunProgress>),
    Done(Vec<FileRunResult>),
}

/// A finished run's `Exported` outcomes hold full file bytes in memory —
/// unlike `ocr_jobs` (which evicts a job on the first poll after it
/// finishes), a run's results are meant to be downloadable more than once,
/// so nothing removes a `Done` entry on read. Left unchecked that's
/// unbounded growth over the life of a long-running server; this keeps at
/// most `MAX_RETAINED_RUNS` finished runs, evicting the oldest one every
/// time a run finishes and the cap is exceeded. A `Running` run is never a
/// candidate — see `ActionRuns::finished_order`.
const MAX_RETAINED_RUNS: usize = 20;

#[derive(Default)]
pub struct ActionRuns {
    entries: HashMap<Uuid, ActionRunState>,
    /// Ids of *finished* runs only, oldest first — a `Running` run is never
    /// pushed here, so it's structurally impossible to evict one out from
    /// under an in-flight job; eviction just pops the front until we're
    /// back under the cap.
    finished_order: VecDeque<Uuid>,
}

impl ActionRuns {
    pub fn insert_running(&mut self, run_id: Uuid, progress: Arc<ActionRunProgress>) {
        self.entries.insert(run_id, ActionRunState::Running(progress));
    }

    pub fn insert_done(&mut self, run_id: Uuid, results: Vec<FileRunResult>) {
        self.entries.insert(run_id, ActionRunState::Done(results));
        self.finished_order.push_back(run_id);
        self.prune();
    }

    pub fn get(&self, run_id: Uuid) -> Option<&ActionRunState> {
        self.entries.get(&run_id)
    }

    fn prune(&mut self) {
        while self.finished_order.len() > MAX_RETAINED_RUNS {
            let Some(oldest) = self.finished_order.pop_front() else { break };
            self.entries.remove(&oldest);
        }
    }
}

pub struct FileRunResult {
    pub source_document_id: Uuid,
    pub outcome: FileRunOutcome,
}

pub enum FileRunOutcome {
    /// Pipeline had no `Export` step — final state is a stored PDF document.
    Document(storage::DocMeta),
    /// Pipeline ended with `Export` — bytes held in memory for download.
    Exported { filename: String, content_type: String, bytes: Vec<u8> },
    /// `step_index` is where it broke; earlier steps for this file already
    /// happened and left intermediate documents behind (not cleaned up —
    /// same as any other partial-failure document in this app).
    Failed { step_index: usize, message: String },
}

/// Run `action` against every document in `document_ids`, one file at a
/// time (the PDFium worker is single-threaded per document anyway, so
/// concurrent files wouldn't actually parallelize — see `PdfEngine`).
pub async fn run_batch(
    state: &SharedState,
    action: &ActionDef,
    document_ids: &[Uuid],
    step_secrets: &HashMap<usize, StepSecrets>,
    progress: &ActionRunProgress,
) -> Vec<FileRunResult> {
    let mut results = Vec::with_capacity(document_ids.len());
    for (file_index, &doc_id) in document_ids.iter().enumerate() {
        progress.current_file.store(file_index, Ordering::Relaxed);
        progress.current_step.store(0, Ordering::Relaxed);
        let outcome = run_one(state, action, doc_id, step_secrets, progress).await;
        results.push(FileRunResult { source_document_id: doc_id, outcome });
    }
    progress.current_file.store(document_ids.len(), Ordering::Relaxed);
    results
}

async fn run_one(
    state: &SharedState,
    action: &ActionDef,
    start_id: Uuid,
    step_secrets: &HashMap<usize, StepSecrets>,
    progress: &ActionRunProgress,
) -> FileRunOutcome {
    let Some(start_meta) = state.storage.get(start_id) else {
        return FileRunOutcome::Failed { step_index: 0, message: "document not found".into() };
    };

    let mut current_id = start_id;
    let mut current_filename = start_meta.filename;

    for (step_index, step) in action.steps.iter().enumerate() {
        progress.current_step.store(step_index, Ordering::Relaxed);
        let secret = step_secrets.get(&step_index);
        match execute_step(state, step, current_id, &current_filename, secret).await {
            Ok(StepOutput::SameDocument) => {}
            Ok(StepOutput::NewDocument(meta)) => {
                current_filename = meta.filename.clone();
                current_id = meta.id;
            }
            Ok(StepOutput::Exported { filename, content_type, bytes }) => {
                return FileRunOutcome::Exported { filename, content_type, bytes };
            }
            Err(message) => return FileRunOutcome::Failed { step_index, message },
        }
    }

    match state.storage.get(current_id) {
        Some(meta) => FileRunOutcome::Document(meta),
        None => FileRunOutcome::Failed {
            step_index: action.steps.len(),
            message: "final document missing".into(),
        },
    }
}

enum StepOutput {
    SameDocument,
    NewDocument(storage::DocMeta),
    Exported { filename: String, content_type: String, bytes: Vec<u8> },
}

fn protect_err_msg(what: &str, e: protect::ProtectError) -> String {
    match e {
        protect::ProtectError::User(msg) => msg,
        protect::ProtectError::Internal(err) => {
            tracing::error!("action step ({what}) failed: {err:#}");
            format!("{what} failed")
        }
    }
}

async fn execute_step(
    state: &SharedState,
    step: &Step,
    doc_id: Uuid,
    current_filename: &str,
    secret: Option<&StepSecrets>,
) -> Result<StepOutput, String> {
    let path = state.storage.pdf_path(doc_id);

    match step.clone() {
        Step::RotateAll { delta } => {
            let job_path = path.clone();
            state
                .engine
                .run(move |pdfium, cache| {
                    pageops::rotate_all(pdfium, &job_path, delta)?;
                    cache.invalidate(&job_path);
                    Ok(())
                })
                .await
                .map_err(|e| e.to_string())?;
            state.storage.bump_revision(doc_id).map_err(|e| e.to_string())?;
            Ok(StepOutput::SameDocument)
        }

        Step::Crop { pages, rect } => {
            let job_path = path.clone();
            state
                .engine
                .run(move |_pdfium, cache| {
                    pageops::crop(&job_path, &pages, rect)?;
                    cache.invalidate(&job_path);
                    Ok(())
                })
                .await
                .map_err(|e| e.to_string())?;
            state.storage.bump_revision(doc_id).map_err(|e| e.to_string())?;
            Ok(StepOutput::SameDocument)
        }

        Step::Resize { pages, width, height, mode } => {
            let job_path = path.clone();
            state
                .engine
                .run(move |_pdfium, cache| {
                    pageops::resize(&job_path, &pages, width, height, mode)?;
                    cache.invalidate(&job_path);
                    Ok(())
                })
                .await
                .map_err(|e| e.to_string())?;
            state.storage.bump_revision(doc_id).map_err(|e| e.to_string())?;
            Ok(StepOutput::SameDocument)
        }

        Step::Compress { preset, dpi, quality } => {
            let (target_dpi, jpeg_quality) = match preset {
                CompressPreset::Screen => (72.0, 60),
                CompressPreset::Ebook => (150.0, 75),
                CompressPreset::Printer => (300.0, 85),
                CompressPreset::Custom => (
                    dpi.unwrap_or(150.0).clamp(36.0, 600.0),
                    quality.unwrap_or(75).clamp(10, 100),
                ),
            };
            let opts = compress::CompressOptions { target_dpi, jpeg_quality };
            let job_path = path.clone();
            let (bytes, _stats) = tokio::task::spawn_blocking(move || compress::compress(&job_path, &opts))
                .await
                .map_err(|e| e.to_string())?
                .map_err(|e| e.to_string())?;
            let new_meta = state
                .storage
                .save(format!("compressed_{current_filename}"), &bytes, None)
                .map_err(|e| e.to_string())?;
            Ok(StepOutput::NewDocument(new_meta))
        }

        Step::Protect { permissions } => {
            let owner_password = secret
                .and_then(|s| s.owner_password.clone())
                .ok_or_else(|| "missing owner_password for protect step".to_string())?;
            let job_path = path.clone();
            let (bytes, hash) = tokio::task::spawn_blocking(move || {
                let hash = protect::hash_password(&owner_password)?;
                let bytes = protect::protect(&job_path, &owner_password, permissions)?;
                Ok::<_, protect::ProtectError>((bytes, hash))
            })
            .await
            .map_err(|e| e.to_string())?
            .map_err(|e| protect_err_msg("protect", e))?;
            let new_meta = state
                .storage
                .save(format!("protected_{current_filename}"), &bytes, Some(hash))
                .map_err(|e| e.to_string())?;
            Ok(StepOutput::NewDocument(new_meta))
        }

        Step::Encrypt { permissions } => {
            let user_password = secret
                .and_then(|s| s.user_password.clone())
                .ok_or_else(|| "missing user_password for encrypt step".to_string())?;
            let owner_password = secret
                .and_then(|s| s.owner_password.clone())
                .unwrap_or_else(|| user_password.clone());
            let flags = permissions.unwrap_or_else(protect::PermissionFlags::all_allowed);
            let job_path = path.clone();
            let bytes = tokio::task::spawn_blocking(move || {
                protect::encrypt(&job_path, &user_password, &owner_password, flags)
            })
            .await
            .map_err(|e| e.to_string())?
            .map_err(|e| protect_err_msg("encrypt", e))?;
            let new_meta = state
                .storage
                .save(format!("encrypted_{current_filename}"), &bytes, None)
                .map_err(|e| e.to_string())?;
            Ok(StepOutput::NewDocument(new_meta))
        }

        Step::Ocr { langs, dpi, min_confidence, force } => {
            let mut opts = ocr::OcrOptions::default();
            if let Some(l) = langs {
                opts.langs = l;
            }
            if let Some(d) = dpi {
                opts.dpi = d.clamp(36.0, 600.0);
            }
            if let Some(mc) = min_confidence {
                opts.min_confidence = mc.clamp(0.0, 100.0);
            }
            opts.force = force;

            let job_path = path.clone();
            let ocr_progress = std::sync::Arc::new(ocr::OcrProgress::default());
            let (bytes, _stats) = state
                .engine
                .run(move |pdfium, _cache| ocr::ocr_document(pdfium, &job_path, &opts, &ocr_progress))
                .await
                .map_err(|e| e.to_string())?;
            let new_meta = state
                .storage
                .save(format!("ocr_{current_filename}"), &bytes, None)
                .map_err(|e| e.to_string())?;
            Ok(StepOutput::NewDocument(new_meta))
        }

        Step::Redact { boxes, dpi, jpeg_quality } => {
            if boxes.is_empty() {
                return Err("redact step needs at least one box".to_string());
            }
            let boxes: Vec<redact::RedactBox> = boxes
                .into_iter()
                .map(|b| redact::RedactBox { page: b.page, x: b.x, y: b.y, w: b.w, h: b.h })
                .collect();
            let mut opts = redact::RedactOptions::default();
            if let Some(d) = dpi {
                opts.dpi = d.clamp(36.0, 600.0);
            }
            if let Some(q) = jpeg_quality {
                opts.jpeg_quality = q.clamp(10, 100);
            }

            let job_path = path.clone();
            let (bytes, _stats) = state
                .engine
                .run(move |pdfium, _cache| redact::redact_document(pdfium, &job_path, &boxes, &opts))
                .await
                .map_err(|e| e.to_string())?;
            let new_meta = state
                .storage
                .save(format!("redacted_{current_filename}"), &bytes, None)
                .map_err(|e| e.to_string())?;
            // Mirrors the single-shot /redact handler: the pre-redaction
            // working copy is deleted once the redacted result is saved.
            if let Err(e) = state.storage.delete(doc_id) {
                tracing::warn!("failed to delete pre-redaction original {doc_id}: {e}");
            }
            Ok(StepOutput::NewDocument(new_meta))
        }

        Step::Export { format, dpi, quality } => {
            let dpi = dpi.unwrap_or(150).clamp(72, 600);
            let quality = quality.unwrap_or(85).clamp(10, 100);
            let scale = dpi as f32 / 72.0;

            let (bytes, content_type, ext) = if let Some(office) = format.as_office() {
                let bytes = sidecar::convert(&path, office, None)
                    .await
                    .map_err(|e| match e {
                        sidecar::SidecarError::User(msg) => msg,
                        sidecar::SidecarError::Internal(err) => {
                            tracing::error!("action export (sidecar) failed: {err:#}");
                            "conversion failed".to_string()
                        }
                    })?;
                (bytes, office.content_type(), office.ext())
            } else {
                let Some(raster) = format.as_raster() else {
                    return Err("invalid export format".to_string());
                };
                let count_path = path.clone();
                let page_count: u16 = state
                    .engine
                    .run(move |pdfium, cache| Ok(cache.open(pdfium, &count_path)?.pages().len()))
                    .await
                    .map_err(|e| e.to_string())?;
                let pages: Vec<u16> = (0..page_count).collect();

                let export_path = path.clone();
                let result = state
                    .engine
                    .run(move |pdfium, cache| {
                        let doc = cache.open(pdfium, &export_path)?;
                        exportops::export(doc, raster, &pages, scale, quality)
                    })
                    .await
                    .map_err(|e| e.to_string())?;
                (result.bytes, result.content_type, result.ext)
            };

            let stem = Path::new(current_filename)
                .file_stem()
                .and_then(|s| s.to_str())
                .unwrap_or("export");
            let filename = format!("{stem}.{ext}");
            Ok(StepOutput::Exported { filename, content_type: content_type.to_string(), bytes })
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmpdir(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("pdfeditor-actions-{tag}-{}", Uuid::new_v4()));
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    fn rotate_step() -> Step {
        Step::RotateAll { delta: 90 }
    }

    fn export_step() -> Step {
        Step::Export { format: ApiExportFormat::Png, dpi: None, quality: None }
    }

    #[test]
    fn validate_steps_rejects_empty() {
        assert!(validate_steps(&[]).is_err());
    }

    #[test]
    fn validate_steps_rejects_export_not_last() {
        let steps = vec![export_step(), rotate_step()];
        let err = validate_steps(&steps).unwrap_err();
        assert!(err.contains("index 0"), "error should name the offending step: {err}");
    }

    #[test]
    fn validate_steps_accepts_export_last() {
        let steps = vec![rotate_step(), export_step()];
        assert!(validate_steps(&steps).is_ok());
    }

    #[test]
    fn validate_secrets_requires_owner_password_for_protect() {
        let steps = vec![Step::Protect { permissions: protect::PermissionFlags::all_allowed() }];
        assert!(validate_secrets(&steps, &HashMap::new()).is_err());

        let mut secrets = HashMap::new();
        secrets.insert(0, StepSecrets { owner_password: Some("hunter2".into()), user_password: None });
        assert!(validate_secrets(&steps, &secrets).is_ok());
    }

    #[test]
    fn validate_secrets_requires_user_password_for_encrypt() {
        let steps = vec![Step::Encrypt { permissions: None }];
        assert!(validate_secrets(&steps, &HashMap::new()).is_err());

        let mut secrets = HashMap::new();
        // Wrong field (owner, not user) — still missing what encrypt needs.
        secrets.insert(0, StepSecrets { owner_password: Some("x".into()), user_password: None });
        assert!(validate_secrets(&steps, &secrets).is_err());

        secrets.get_mut(&0).unwrap().user_password = Some("hunter2".into());
        assert!(validate_secrets(&steps, &secrets).is_ok());
    }

    #[test]
    fn validate_secrets_ignores_steps_that_dont_need_one() {
        let steps = vec![rotate_step(), export_step()];
        assert!(validate_secrets(&steps, &HashMap::new()).is_ok());
    }

    #[test]
    fn store_create_get_list_delete_roundtrip() {
        let store = ActionStore::new(tmpdir("crud")).unwrap();
        let def = store.create("test action".into(), vec![rotate_step()]).unwrap();

        assert_eq!(store.get(def.id).unwrap().name, "test action");
        assert_eq!(store.list().len(), 1);

        store.delete(def.id).unwrap();
        assert!(store.get(def.id).is_none());
        assert!(store.list().is_empty());
        assert!(store.delete(def.id).is_err(), "deleting twice should error, not panic");
    }

    #[test]
    fn store_create_rejects_invalid_steps() {
        let store = ActionStore::new(tmpdir("invalid")).unwrap();
        assert!(store.create("bad".into(), vec![]).is_err());
    }

    #[test]
    fn store_reloads_persisted_actions_on_restart() {
        let dir = tmpdir("reload");
        let def_id = {
            let store = ActionStore::new(&dir).unwrap();
            store.create("survives restart".into(), vec![rotate_step(), export_step()]).unwrap().id
        };

        let reopened = ActionStore::new(&dir).unwrap();
        let def = reopened.get(def_id).expect("action should survive a fresh ActionStore::new over the same dir");
        assert_eq!(def.name, "survives restart");
        assert_eq!(def.steps.len(), 2);
    }

    fn done(run_id: Uuid, runs: &mut ActionRuns) {
        runs.insert_done(run_id, Vec::new());
    }

    #[test]
    fn action_runs_prunes_oldest_finished_past_the_cap() {
        let mut runs = ActionRuns::default();
        let ids: Vec<Uuid> = (0..MAX_RETAINED_RUNS + 5).map(|_| Uuid::new_v4()).collect();
        for &id in &ids {
            done(id, &mut runs);
        }

        // The oldest 5 (past the cap) should be gone; the newest MAX_RETAINED_RUNS kept.
        for &id in &ids[..5] {
            assert!(runs.get(id).is_none(), "oldest finished run should have been pruned");
        }
        for &id in &ids[5..] {
            assert!(runs.get(id).is_some(), "run within the retention window should still be there");
        }
    }

    #[test]
    fn action_runs_never_evicts_a_still_running_run() {
        let mut runs = ActionRuns::default();
        let running_id = Uuid::new_v4();
        let progress = Arc::new(ActionRunProgress::new(1, 1));
        runs.insert_running(running_id, progress);

        // Push well past the cap with finished runs — the still-running one
        // (oldest by insertion order) must survive regardless.
        for _ in 0..MAX_RETAINED_RUNS + 10 {
            done(Uuid::new_v4(), &mut runs);
        }

        assert!(
            matches!(runs.get(running_id), Some(ActionRunState::Running(_))),
            "a Running run must never be evicted, even if it's the oldest entry"
        );
    }
}
