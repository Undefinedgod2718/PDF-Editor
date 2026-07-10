//! PDFium is not thread-safe, so all PDFium work runs on one dedicated
//! worker thread. Requests are sent as boxed closures over a channel and
//! answered through oneshot channels.

use pdfium_render::prelude::*;
use tokio::sync::{mpsc, oneshot};

type Job = Box<dyn FnOnce(&Pdfium) + Send>;

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
                let bindings = Pdfium::bind_to_library(
                    Pdfium::pdfium_platform_library_name_at_path("./"),
                )
                .or_else(|_| Pdfium::bind_to_system_library());

                let pdfium = match bindings {
                    Ok(b) => {
                        let _ = ready_tx.send(Ok(()));
                        Pdfium::new(b)
                    }
                    Err(e) => {
                        let _ = ready_tx.send(Err(e.to_string()));
                        return;
                    }
                };

                while let Some(job) = rx.blocking_recv() {
                    job(&pdfium);
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
        F: FnOnce(&Pdfium) -> anyhow::Result<T> + Send + 'static,
    {
        let (tx, rx) = oneshot::channel();
        self.tx
            .send(Box::new(move |pdfium| {
                let _ = tx.send(f(pdfium));
            }))
            .map_err(|_| anyhow::anyhow!("pdfium worker thread is gone"))?;
        rx.await?
    }
}
