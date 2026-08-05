pub mod annots;
pub mod compare;
pub mod compress;
mod docutil;
pub mod embedfont;
pub mod engine;
pub mod exportops;
pub mod fingerprint;
pub mod font;
pub mod formbuild;
pub mod formops;
pub mod imageops;
pub mod links;
pub mod objects;
pub mod ocr;
pub mod ops;
pub mod outline;

pub mod pageops;
pub mod protect;
pub mod redact;
pub mod stamptext;
pub mod textedit;

use std::path::Path;

use pdfium_render::prelude::*;

/// Load, mutate, and atomically save a document back to `path`.
/// The document is loaded from an owned byte buffer so no file handle
/// is held while we overwrite the file.
///
/// Refuses protected PDFs: a PDFium load+save of an empty-user-password
/// document strips `/Encrypt` and would silently unlock the file on disk.
pub(crate) fn with_document<T>(
    pdfium: &Pdfium,
    path: &Path,
    f: impl FnOnce(&mut PdfDocument) -> anyhow::Result<T>,
) -> anyhow::Result<T> {
    protect::assert_editable(path)?;
    let bytes = std::fs::read(path)?;
    let mut doc = pdfium.load_pdf_from_byte_vec(bytes, None)?;
    let result = f(&mut doc)?;
    let saved = doc.save_to_bytes()?;
    drop(doc);
    let tmp = path.with_extension("pdf.tmp");
    std::fs::write(&tmp, &saved)?;
    std::fs::rename(&tmp, path)?;
    Ok(result)
}
