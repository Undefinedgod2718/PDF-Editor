//! Page-level operations: rotate, delete, insert, reorder, merge, extract.
//! Reorder/merge/extract build a fresh document and copy pages across,
//! because PDFium has no in-place page-move API.

use std::path::Path;

use pdfium_render::prelude::*;

use super::with_document;

const DEFAULT_PAGE_W: f32 = 612.0; // US Letter, points
const DEFAULT_PAGE_H: f32 = 792.0;

/// Set absolute page rotation. Accepts 0 / 90 / 180 / 270.
pub fn rotate(pdfium: &Pdfium, path: &Path, page_index: u16, degrees: u16) -> anyhow::Result<()> {
    let rotation = match degrees {
        0 => PdfPageRenderRotation::None,
        90 => PdfPageRenderRotation::Degrees90,
        180 => PdfPageRenderRotation::Degrees180,
        270 => PdfPageRenderRotation::Degrees270,
        d => anyhow::bail!("unsupported rotation {d}, use 0/90/180/270"),
    };
    with_document(pdfium, path, |doc| {
        let mut page = doc.pages().get(page_index)?;
        page.set_rotation(rotation);
        Ok(())
    })
}

pub fn delete_page(pdfium: &Pdfium, path: &Path, page_index: u16) -> anyhow::Result<()> {
    with_document(pdfium, path, |doc| {
        if doc.pages().len() <= 1 {
            anyhow::bail!("cannot delete the only remaining page");
        }
        doc.pages().get(page_index)?.delete()?;
        Ok(())
    })
}

/// Insert a blank page at `at`. Size defaults to the page currently at that
/// position (or the last page when appending), falling back to US Letter.
pub fn insert_blank(
    pdfium: &Pdfium,
    path: &Path,
    at: u16,
    width: Option<f32>,
    height: Option<f32>,
) -> anyhow::Result<()> {
    with_document(pdfium, path, |doc| {
        let count = doc.pages().len();
        if at > count {
            anyhow::bail!("insert index {at} out of range (0..={count})");
        }
        let (mut w, mut h) = (DEFAULT_PAGE_W, DEFAULT_PAGE_H);
        let neighbor = if at < count {
            Some(at)
        } else if count > 0 {
            Some(count - 1)
        } else {
            None
        };
        if let Some(n) = neighbor {
            let p = doc.pages().get(n)?;
            w = p.width().value;
            h = p.height().value;
        }
        let size = PdfPagePaperSize::Custom(
            PdfPoints::new(width.unwrap_or(w)),
            PdfPoints::new(height.unwrap_or(h)),
        );
        doc.pages_mut().create_page_at_index(size, at)?;
        Ok(())
    })
}

/// Reorder pages. `order` must be a permutation of 0..page_count.
pub fn reorder(pdfium: &Pdfium, path: &Path, order: &[u16]) -> anyhow::Result<()> {
    let bytes = std::fs::read(path)?;
    let src = pdfium.load_pdf_from_byte_vec(bytes, None)?;
    let count = src.pages().len();

    let mut seen = vec![false; count as usize];
    if order.len() != count as usize {
        anyhow::bail!("order length {} != page count {count}", order.len());
    }
    for &i in order {
        if i >= count || seen[i as usize] {
            anyhow::bail!("order is not a permutation of 0..{count}");
        }
        seen[i as usize] = true;
    }

    let mut dest = pdfium.create_new_pdf()?;
    for (dest_index, &src_index) in order.iter().enumerate() {
        dest.pages_mut()
            .copy_page_from_document(&src, src_index, dest_index as u16)?;
    }
    let saved = dest.save_to_bytes()?;
    drop(dest);
    drop(src);
    let tmp = path.with_extension("pdf.tmp");
    std::fs::write(&tmp, &saved)?;
    std::fs::rename(&tmp, path)?;
    Ok(())
}

/// Merge documents in the given order into a new PDF, returning its bytes.
pub fn merge(pdfium: &Pdfium, paths: &[std::path::PathBuf]) -> anyhow::Result<Vec<u8>> {
    if paths.len() < 2 {
        anyhow::bail!("merge needs at least two documents");
    }
    let mut dest = pdfium.create_new_pdf()?;
    for p in paths {
        let bytes = std::fs::read(p)?;
        let src = pdfium.load_pdf_from_byte_vec(bytes, None)?;
        dest.pages_mut().append(&src)?;
    }
    Ok(dest.save_to_bytes()?)
}

/// Extract the given pages (in the given order) into a new PDF, returning
/// its bytes. This doubles as "split": call once per desired output.
pub fn extract(pdfium: &Pdfium, path: &Path, pages: &[u16]) -> anyhow::Result<Vec<u8>> {
    if pages.is_empty() {
        anyhow::bail!("extract needs at least one page");
    }
    let bytes = std::fs::read(path)?;
    let src = pdfium.load_pdf_from_byte_vec(bytes, None)?;
    let count = src.pages().len();
    for &i in pages {
        if i >= count {
            anyhow::bail!("page index {i} out of range (0..{count})");
        }
    }
    let mut dest = pdfium.create_new_pdf()?;
    for (dest_index, &src_index) in pages.iter().enumerate() {
        dest.pages_mut()
            .copy_page_from_document(&src, src_index, dest_index as u16)?;
    }
    Ok(dest.save_to_bytes()?)
}
