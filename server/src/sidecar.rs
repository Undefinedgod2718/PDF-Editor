//! Python sidecar invocation for PDF → Office (docx/xlsx) conversion.
//!
//! The sidecar is a uv-managed Python project (`python/convert.py`, pdf2docx +
//! pdfplumber). Contract: single JSON line `{"ok":true,"pages":N}` on stdout /
//! exit 0, or `{"ok":false,"error":"..."}` as the LAST stderr line / exit != 0.
//! Interpreter and script paths resolve from `PDF_EDITOR_PYTHON` /
//! `PDF_EDITOR_SIDECAR` env vars, falling back to dev (`../python/.venv`) and
//! deployment (`python/python.exe` embedded zip next to the exe cwd) layouts.

use std::path::{Path, PathBuf};
use std::time::Duration;

use serde::Deserialize;
use tokio::process::Command;

/// Formats converted by the Python sidecar rather than the PDFium worker.
#[derive(Clone, Copy)]
pub enum OfficeFormat {
    Docx,
    Xlsx,
}

impl OfficeFormat {
    fn mode(self) -> &'static str {
        match self {
            OfficeFormat::Docx => "docx",
            OfficeFormat::Xlsx => "xlsx",
        }
    }

    pub fn ext(self) -> &'static str {
        self.mode()
    }

    pub fn content_type(self) -> &'static str {
        match self {
            OfficeFormat::Docx => {
                "application/vnd.openxmlformats-officedocument.wordprocessingml.document"
            }
            OfficeFormat::Xlsx => {
                "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet"
            }
        }
    }
}

/// Sidecar failure split by blame, so the API layer can pick a status code.
/// `User` carries the sidecar's own validation message (encrypted document,
/// unparseable PDF, ...); `Internal` is a crash, timeout, or broken install.
pub enum SidecarError {
    User(String),
    Internal(anyhow::Error),
}

impl From<anyhow::Error> for SidecarError {
    fn from(e: anyhow::Error) -> Self {
        SidecarError::Internal(e)
    }
}

#[derive(Deserialize)]
struct SidecarStatus {
    ok: bool,
    #[serde(default)]
    error: String,
}

fn first_existing(candidates: &[PathBuf]) -> Option<PathBuf> {
    candidates.iter().find(|p| p.is_file()).cloned()
}

fn resolve(env_var: &str, candidates: &[&str], what: &str) -> anyhow::Result<PathBuf> {
    if let Ok(p) = std::env::var(env_var) {
        let p = PathBuf::from(p);
        anyhow::ensure!(p.is_file(), "{env_var}={} does not exist", p.display());
        return Ok(p);
    }
    let paths: Vec<PathBuf> = candidates.iter().map(PathBuf::from).collect();
    first_existing(&paths).ok_or_else(|| {
        anyhow::anyhow!(
            "sidecar {what} not found; set {env_var} or install one of: {}",
            candidates.join(", ")
        )
    })
}

fn resolve_python() -> anyhow::Result<PathBuf> {
    resolve(
        "PDF_EDITOR_PYTHON",
        &[
            // dev: cwd = server/
            "../python/.venv/Scripts/python.exe",
            // cwd = repo root
            "python/.venv/Scripts/python.exe",
            // deployment: embedded Python zip at C:\PDFEditor\python
            "python/python.exe",
        ],
        "python interpreter",
    )
}

fn resolve_script() -> anyhow::Result<PathBuf> {
    resolve(
        "PDF_EDITOR_SIDECAR",
        &["../python/convert.py", "python/convert.py"],
        "convert.py",
    )
}

/// Hard cap on a single conversion; pdf2docx on a large scanned document can
/// crawl, and a wedged subprocess must not pin the request forever.
const TIMEOUT: Duration = Duration::from_secs(300);

/// Convert `pdf_path` to `format`. `pages` are 0-based indices already
/// validated by the caller; `None` means all pages. Returns the output bytes.
pub async fn convert(
    pdf_path: &Path,
    format: OfficeFormat,
    pages: Option<&[u16]>,
) -> Result<Vec<u8>, SidecarError> {
    let python = resolve_python()?;
    let script = resolve_script()?;

    let out_path = std::env::temp_dir().join(format!(
        "pdfeditor-{}.{}",
        uuid::Uuid::new_v4(),
        format.ext()
    ));

    let mut cmd = Command::new(&python);
    cmd.arg(&script)
        .arg("--mode")
        .arg(format.mode())
        .arg("--input")
        .arg(pdf_path)
        .arg("--output")
        .arg(&out_path);
    if let Some(pages) = pages {
        let list: Vec<String> = pages.iter().map(u16::to_string).collect();
        cmd.arg("--pages").arg(list.join(","));
    }
    cmd.kill_on_drop(true);
    #[cfg(windows)]
    cmd.creation_flags(0x0800_0000); // CREATE_NO_WINDOW: no console flash under a service

    let result = tokio::time::timeout(TIMEOUT, cmd.output()).await;
    let output = match result {
        Err(_) => {
            let _ = tokio::fs::remove_file(&out_path).await;
            return Err(SidecarError::Internal(anyhow::anyhow!(
                "conversion timed out after {}s",
                TIMEOUT.as_secs()
            )));
        }
        Ok(io) => io.map_err(|e| anyhow::anyhow!("failed to spawn sidecar: {e}"))?,
    };

    if !output.status.success() {
        let _ = tokio::fs::remove_file(&out_path).await;
        let stderr = String::from_utf8_lossy(&output.stderr);
        // Contract: last stderr line is the JSON error report.
        let status = stderr
            .lines()
            .rev()
            .find(|l| !l.trim().is_empty())
            .and_then(|l| serde_json::from_str::<SidecarStatus>(l.trim()).ok());
        return Err(match status {
            Some(s) if !s.ok && !s.error.is_empty() => SidecarError::User(s.error),
            _ => SidecarError::Internal(anyhow::anyhow!(
                "sidecar exited with {}: {}",
                output.status,
                stderr.trim()
            )),
        });
    }

    let bytes = tokio::fs::read(&out_path).await.map_err(|e| {
        anyhow::anyhow!("sidecar reported success but output is unreadable: {e}")
    });
    let _ = tokio::fs::remove_file(&out_path).await;
    Ok(bytes?)
}
