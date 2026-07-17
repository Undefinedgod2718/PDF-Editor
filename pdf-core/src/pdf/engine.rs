//! PDFium is not thread-safe, so all PDFium work runs on one dedicated
//! worker thread. Requests are sent as boxed closures over a channel and
//! answered through oneshot channels.
//!
//! The worker also owns a small LRU cache of open documents so read
//! endpoints (render, text, search, …) don't re-parse the PDF on every
//! request. Mutating endpoints must call [`DocCache::invalidate`] after
//! rewriting the file.

use std::num::NonZeroUsize;
use std::path::{Path, PathBuf};

use lru::LruCache;
use pdfium_render::prelude::*;
use tokio::sync::{mpsc, oneshot};

/// Documents the worker keeps open, keyed by file path.
///
/// Entries are loaded from an owned byte buffer — never from an open file
/// handle — so a mutation can atomically rename over the underlying path
/// (see `with_document`) while a cached copy of the old bytes still exists.
pub struct DocCache {
    docs: LruCache<PathBuf, PdfDocument<'static>>,
}

impl DocCache {
    fn new() -> Self {
        Self {
            // Each entry holds the whole file in memory, so keep this small.
            docs: LruCache::new(NonZeroUsize::new(8).unwrap()),
        }
    }

    /// Return the cached document for `path`, loading and caching it on miss.
    pub fn open(
        &mut self,
        pdfium: &'static Pdfium,
        path: &Path,
    ) -> anyhow::Result<&mut PdfDocument<'static>> {
        if !self.docs.contains(path) {
            let bytes = std::fs::read(path)?;
            let doc = pdfium.load_pdf_from_byte_vec(bytes, None)?;
            self.docs.put(path.to_path_buf(), doc);
        }
        Ok(self.docs.get_mut(path).expect("just inserted"))
    }

    /// Drop the cached copy after the file at `path` has been rewritten.
    pub fn invalidate(&mut self, path: &Path) {
        self.docs.pop(path);
    }
}

type Job = Box<dyn FnOnce(&'static Pdfium, &mut DocCache) + Send>;

pub struct PdfEngine {
    tx: mpsc::UnboundedSender<Job>,
}

impl PdfEngine {
    pub fn spawn() -> anyhow::Result<Self> {
        let (tx, mut rx) = mpsc::unbounded_channel::<Job>();
        let (ready_tx, ready_rx) = std::sync::mpsc::channel::<Result<(), String>>();

        std::thread::Builder::new()
            .name("pdfium-worker".into())
            .spawn(move || {
                // 順序：PDF_EDITOR_PDFIUM 指定目錄 → 執行檔所在目錄
                // （桌面版 bundle、deploy BIN）→ cwd（server 既有慣例）
                // → 系統庫。
                let mut candidates: Vec<String> = Vec::new();
                if let Ok(dir) = std::env::var("PDF_EDITOR_PDFIUM") {
                    candidates.push(dir);
                }
                if let Some(exe_dir) = std::env::current_exe()
                    .ok()
                    .and_then(|p| p.parent().map(|d| d.to_path_buf()))
                {
                    candidates.push(exe_dir.to_string_lossy().into_owned());
                }
                candidates.push("./".into());

                let mut bindings = None;
                for dir in &candidates {
                    if let Ok(b) = Pdfium::bind_to_library(
                        Pdfium::pdfium_platform_library_name_at_path(dir),
                    ) {
                        bindings = Some(b);
                        break;
                    }
                }
                let bindings = match bindings {
                    Some(b) => Ok(b),
                    None => Pdfium::bind_to_system_library(),
                };

                let pdfium = match bindings {
                    Ok(b) => {
                        let _ = ready_tx.send(Ok(()));
                        // Leaked so cached documents can borrow it as 'static.
                        // One instance per process, alive for the process
                        // lifetime anyway.
                        &*Box::leak(Box::new(Pdfium::new(b)))
                    }
                    Err(e) => {
                        let _ = ready_tx.send(Err(e.to_string()));
                        return;
                    }
                };

                let mut cache = DocCache::new();
                while let Some(job) = rx.blocking_recv() {
                    job(pdfium, &mut cache);
                }
            })?;

        ready_rx
            .recv()?
            .map_err(|e| anyhow::anyhow!("failed to load pdfium library: {e}"))?;

        Ok(Self { tx })
    }

    /// Run a closure on the PDFium worker thread and await its result.
    pub async fn run<T, F>(&self, f: F) -> anyhow::Result<T>
    where
        T: Send + 'static,
        F: FnOnce(&'static Pdfium, &mut DocCache) -> anyhow::Result<T> + Send + 'static,
    {
        let (tx, rx) = oneshot::channel();
        self.tx
            .send(Box::new(move |pdfium, cache| {
                let _ = tx.send(f(pdfium, cache));
            }))
            .map_err(|_| anyhow::anyhow!("pdfium worker thread is gone"))?;
        rx.await?
    }
}
