pub mod annots;
pub mod engine;
pub mod font;
pub mod formops;
pub mod objects;
pub mod ops;
pub mod pageops;

use std::path::Path;

use pdfium_render::prelude::*;

/// Load, mutate, and atomically save a document back to `path`.
/// The document is loaded from an owned byte buffer so no file handle
/// is held while we overwrite the file.
pub(crate) fn with_document<T>(
    pdfium: &Pdfium,
    path: &Path,
    f: impl FnOnce(&mut PdfDocument) -> anyhow::Result<T>,
) -> anyhow::Result<T> {
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
