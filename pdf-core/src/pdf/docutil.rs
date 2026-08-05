//! Low-level lopdf helpers shared by the modules that do dictionary surgery
//! (`outline`, `links`, `stamptext`).
//!
//! `formbuild` and `pageops` predate this module and keep private copies of
//! some of these; they are left alone deliberately — this is the home for
//! new callers, not a refactor of the existing ones.

use std::path::Path;

use lopdf::{Dictionary, Document, Object, ObjectId};

pub(crate) fn catalog_id(doc: &Document) -> anyhow::Result<ObjectId> {
    doc.trailer
        .get(b"Root")
        .and_then(|o| o.as_reference())
        .map_err(|_| anyhow::anyhow!("trailer /Root missing"))
}

/// Page object ids in document order (index 0 = first page).
pub(crate) fn page_ids(doc: &Document) -> Vec<ObjectId> {
    let pages = doc.get_pages();
    let mut ordered: Vec<(u32, ObjectId)> = pages.into_iter().collect();
    ordered.sort_by_key(|(number, _)| *number);
    ordered.into_iter().map(|(_, id)| id).collect()
}

pub(crate) fn page_id(doc: &Document, page_index: u16) -> anyhow::Result<ObjectId> {
    doc.get_pages()
        .get(&(page_index as u32 + 1))
        .copied()
        .ok_or_else(|| anyhow::anyhow!("page {page_index} out of range"))
}

pub(crate) fn resolve_array(doc: &Document, obj: &Object) -> Option<Vec<Object>> {
    match obj {
        Object::Array(a) => Some(a.clone()),
        Object::Reference(id) => doc
            .get_object(*id)
            .ok()
            .and_then(|o| o.as_array().ok().cloned()),
        _ => None,
    }
}

pub(crate) fn resolve_dict(doc: &Document, obj: &Object) -> Option<Dictionary> {
    match obj {
        Object::Dictionary(d) => Some(d.clone()),
        Object::Reference(id) => doc.get_dictionary(*id).ok().cloned(),
        _ => None,
    }
}

pub(crate) fn to_f32(obj: &Object) -> Option<f32> {
    match obj {
        Object::Integer(i) => Some(*i as f32),
        Object::Real(r) => Some(*r),
        _ => None,
    }
}

/// MediaBox resolved through page-tree inheritance, normalized to
/// `[x0, y0, x1, y1]` with x0<x1, y0<y1.
pub(crate) fn media_box(doc: &Document, page_id: ObjectId) -> anyhow::Result<[f32; 4]> {
    let mut current = Some(page_id);
    // Malformed files can loop /Parent back on themselves; the page tree is
    // never this deep in practice.
    for _ in 0..64 {
        let Some(id) = current else { break };
        let dict = doc.get_dictionary(id)?;
        if let Some(arr) = dict.get(b"MediaBox").ok().and_then(|o| resolve_array(doc, o)) {
            let v: Vec<f32> = arr.iter().filter_map(to_f32).collect();
            if v.len() != 4 {
                anyhow::bail!("malformed /MediaBox");
            }
            return Ok([
                v[0].min(v[2]),
                v[1].min(v[3]),
                v[0].max(v[2]),
                v[1].max(v[3]),
            ]);
        }
        current = dict.get(b"Parent").ok().and_then(|p| p.as_reference().ok());
    }
    anyhow::bail!("page has no /MediaBox")
}

/// Encode per PDF 32000-1 §7.9.2.2: plain literal when ASCII, otherwise
/// UTF-16BE with BOM. Never write raw UTF-8 into a text string.
pub(crate) fn pdf_text_string(s: &str) -> Object {
    if s.is_ascii() {
        return Object::string_literal(s);
    }
    let mut bytes = vec![0xFE, 0xFF];
    for unit in s.encode_utf16() {
        bytes.extend_from_slice(&unit.to_be_bytes());
    }
    Object::String(bytes, lopdf::StringFormat::Hexadecimal)
}

pub(crate) fn decode_text_string(doc: &Document, obj: &Object) -> Option<String> {
    let obj = match obj {
        Object::Reference(id) => doc.get_object(*id).ok()?,
        other => other,
    };
    let Object::String(bytes, _) = obj else {
        return None;
    };
    if bytes.starts_with(&[0xFE, 0xFF]) {
        let units: Vec<u16> = bytes[2..]
            .chunks_exact(2)
            .map(|c| u16::from_be_bytes([c[0], c[1]]))
            .collect();
        return String::from_utf16(&units).ok();
    }
    Some(String::from_utf8_lossy(bytes).into_owned())
}

/// Append `annot_id` to the page's /Annots, normalizing an inherited or
/// indirect array onto the page dictionary.
pub(crate) fn push_page_annot(
    doc: &mut Document,
    page_id: ObjectId,
    annot_id: ObjectId,
) -> anyhow::Result<()> {
    let page = doc.get_dictionary(page_id)?;
    let mut annots = page
        .get(b"Annots")
        .ok()
        .and_then(|a| resolve_array(doc, a))
        .unwrap_or_default();
    annots.push(Object::Reference(annot_id));
    doc.get_dictionary_mut(page_id)?.set("Annots", annots);
    Ok(())
}

pub(crate) fn save_atomic(doc: &mut Document, path: &Path) -> anyhow::Result<()> {
    let mut bytes = Vec::new();
    doc.save_to(&mut bytes)?;
    let tmp = path.with_extension("pdf.tmp");
    std::fs::write(&tmp, &bytes)?;
    std::fs::rename(&tmp, path)?;
    Ok(())
}

#[cfg(test)]
pub(crate) mod test_support {
    use lopdf::{dictionary, Document, Object};

    /// Minimal `pages`-page PDF (612×792) written to a unique temp file.
    pub(crate) fn temp_pdf(pages: usize) -> std::path::PathBuf {
        let mut doc = Document::with_version("1.5");
        let pages_id = doc.new_object_id();
        let kids: Vec<Object> = (0..pages)
            .map(|_| {
                Object::Reference(doc.add_object(dictionary! {
                    "Type" => "Page",
                    "Parent" => Object::Reference(pages_id),
                    "MediaBox" => vec![0.into(), 0.into(), 612.into(), 792.into()],
                }))
            })
            .collect();
        doc.objects.insert(
            pages_id,
            Object::Dictionary(dictionary! {
                "Type" => "Pages",
                "Kids" => kids,
                "Count" => pages as i64,
            }),
        );
        let catalog = doc.add_object(dictionary! {
            "Type" => "Catalog",
            "Pages" => Object::Reference(pages_id),
        });
        doc.trailer.set("Root", Object::Reference(catalog));
        let path = std::env::temp_dir().join(format!("pdfcore_test_{}.pdf", uuid::Uuid::new_v4()));
        doc.save(&path).unwrap();
        path
    }
}
