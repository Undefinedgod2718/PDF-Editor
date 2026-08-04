//! Python sidecar invocation for PDF → Office/Markdown (docx/xlsx/md) conversion.
//!
//! The sidecar is a uv-managed Python project (`python/convert.py`, pdf2docx +
//! pdfplumber + MarkItDown). Contract: single JSON line `{"ok":true,"pages":N}` on stdout /
//! exit 0, or `{"ok":false,"error":"..."}` as the LAST stderr line / exit != 0.
//! Interpreter and script paths resolve from `PDF_EDITOR_PYTHON` /
//! `PDF_EDITOR_SIDECAR` env vars, else by probing dev (`../python/.venv`) and
//! deployment (`python/python.exe`, the embeddable build) layouts under both
//! the exe's own directory and the cwd — see `search_roots`.

use std::path::{Path, PathBuf};
use std::time::Duration;

use serde::Deserialize;
use tokio::process::Command;

/// Formats converted by the Python sidecar rather than the PDFium worker.
#[derive(Clone, Copy)]
pub enum OfficeFormat {
    Docx,
    Xlsx,
    /// Whole-document only — MarkItDown has no per-page API. Callers must
    /// reject a non-empty page selection before reaching `convert`.
    Markdown,
}

impl OfficeFormat {
    fn mode(self) -> &'static str {
        match self {
            OfficeFormat::Docx => "docx",
            OfficeFormat::Xlsx => "xlsx",
            OfficeFormat::Markdown => "markdown",
        }
    }

    pub fn ext(self) -> &'static str {
        match self {
            OfficeFormat::Docx => "docx",
            OfficeFormat::Xlsx => "xlsx",
            OfficeFormat::Markdown => "md",
        }
    }

    pub fn content_type(self) -> &'static str {
        match self {
            OfficeFormat::Docx => {
                "application/vnd.openxmlformats-officedocument.wordprocessingml.document"
            }
            OfficeFormat::Xlsx => {
                "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet"
            }
            OfficeFormat::Markdown => "text/markdown; charset=utf-8",
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

/// Roots that relative candidates are resolved against, in priority order:
/// the directory holding the running executable, then the process cwd.
///
/// exe_dir has to be in here at all, and has to come first. Resolving a bare
/// relative candidate uses only the cwd, which for a desktop install is
/// whatever directory happened to launch the app — the Start Menu / desktop
/// shortcuts pin it to INSTALLDIR (`WorkingDirectory` in
/// desktop/windows/main.wxs), but the file association launches
/// `"<exe>" "%1"` with no working directory, so opening a PDF by double-click
/// inherits Explorer's cwd. Same install, same binary, and `python/python.exe`
/// would resolve only on some launches — a far worse failure mode than never.
///
/// Mirrors `pdf::ocr::tessdata_dir`'s order (whose doc comment already claimed
/// this function worked this way).
fn search_roots() -> Vec<PathBuf> {
    let mut roots = Vec::new();
    if let Some(exe_dir) = std::env::current_exe()
        .ok()
        .and_then(|p| p.parent().map(Path::to_path_buf))
    {
        roots.push(exe_dir);
    }
    roots.push(PathBuf::from("."));
    roots
}

fn resolve(env_var: &str, candidates: &[&str], what: &str) -> anyhow::Result<PathBuf> {
    if let Ok(p) = std::env::var(env_var) {
        let p = PathBuf::from(p);
        anyhow::ensure!(p.is_file(), "{env_var}={} does not exist", p.display());
        return Ok(p);
    }
    let paths: Vec<PathBuf> = search_roots()
        .iter()
        .flat_map(|root| candidates.iter().map(|c| root.join(c)))
        .collect();
    first_existing(&paths).ok_or_else(|| {
        // List what was actually probed, not the bare candidate strings: the
        // whole point of this resolution is that the same candidate means
        // different files depending on the root, so "not found" is only
        // actionable with the roots spelled out.
        let tried: Vec<String> = paths.iter().map(|p| p.display().to_string()).collect();
        anyhow::anyhow!(
            "sidecar {what} not found; set {env_var} or install one of: {}",
            tried.join(", ")
        )
    })
}

// Each candidate below is tried under every root from `search_roots` — so
// "cwd = server/" style notes describe the dev case, while the deployment case
// is the same string resolved against the exe's own directory.
fn resolve_python() -> anyhow::Result<PathBuf> {
    resolve(
        "PDF_EDITOR_PYTHON",
        &[
            // dev: cwd = server/
            "../python/.venv/Scripts/python.exe",
            "../python/.venv/bin/python",
            // cwd = repo root
            "python/.venv/Scripts/python.exe",
            "python/.venv/bin/python",
            // deployment: embeddable Python next to the exe, built by
            // desktop/windows/prepare-python-embed.ps1 and bundled into
            // INSTALLDIR\python by tauri.conf.json's resources map.
            "python/python.exe",
        ],
        "python interpreter",
    )
}

fn resolve_script() -> anyhow::Result<PathBuf> {
    resolve(
        "PDF_EDITOR_SIDECAR",
        // Deployment ships convert.py as a sibling of python.exe, so the
        // second candidate covers INSTALLDIR\python\convert.py too.
        &["../python/convert.py", "python/convert.py"],
        "convert.py",
    )
}

/// Startup / health probe: report whether the office-conversion sidecar is
/// usable, so a broken install surfaces at boot and in `/api/health` instead
/// of as a 500 on the first export request. Returns the resolved paths.
pub fn health() -> Result<(PathBuf, PathBuf), String> {
    let python = resolve_python().map_err(|e| e.to_string())?;
    let script = resolve_script().map_err(|e| e.to_string())?;
    Ok((python, script))
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

#[cfg(test)]
mod tests {
    use super::*;

    /// The exe's own directory must be probed, and probed before the cwd.
    /// Dropping it regresses the desktop install to "markdown/docx export works
    /// from the Start Menu shortcut but not when you double-click a PDF",
    /// because only the shortcut pins the cwd to INSTALLDIR.
    #[test]
    fn search_roots_probes_exe_dir_first() {
        let roots = search_roots();
        let exe_dir = std::env::current_exe()
            .expect("test binary has a path")
            .parent()
            .expect("test binary has a parent dir")
            .to_path_buf();
        assert_eq!(roots.first(), Some(&exe_dir), "exe dir must be probed first");
        assert!(roots.contains(&PathBuf::from(".")), "cwd must still be probed");
    }

    /// A missing sidecar has to name the paths it actually probed — the whole
    /// failure mode this resolution fixes is one candidate string meaning
    /// different files under different roots.
    #[test]
    fn missing_sidecar_error_lists_probed_paths() {
        let err = resolve(
            "PDF_EDITOR_NONEXISTENT_TEST_VAR",
            &["definitely/not/here.exe"],
            "python interpreter",
        )
        .expect_err("nothing should resolve");
        let msg = err.to_string();
        let exe_dir = std::env::current_exe().unwrap().parent().unwrap().to_path_buf();
        assert!(
            msg.contains(&exe_dir.join("definitely/not/here.exe").display().to_string()),
            "error should name the exe-dir path it probed, got: {msg}"
        );
    }
}
