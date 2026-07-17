//! Page content editing: list / edit / delete the text objects on a page.
//! Object `index` refers to the object's position in the page's full object
//! collection (all object kinds), so indices stay stable across the list
//! and mutation endpoints as long as the page isn't otherwise modified.

use std::path::Path;

use pdfium_render::prelude::*;
use serde::Serialize;

use super::with_document;

#[derive(Serialize)]
pub struct TextObjectInfo {
    pub index: usize,
    pub text: String,
    /// Bounding box in points, top-left origin.
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
    pub font_size: f32,
}

pub fn list_text_objects(
    doc: &PdfDocument,
    page_index: u16,
) -> anyhow::Result<Vec<TextObjectInfo>> {
    let page = doc.pages().get(page_index)?;
    let page_height = page.height().value;
    let mut out = Vec::new();
    for (index, object) in page.objects().iter().enumerate() {
        if let Some(text_obj) = object.as_text_object() {
            let bounds = object.bounds()?;
            out.push(TextObjectInfo {
                index,
                text: text_obj.text(),
                x: bounds.left().value,
                y: page_height - bounds.top().value,
                w: bounds.right().value - bounds.left().value,
                h: bounds.top().value - bounds.bottom().value,
                font_size: text_obj.scaled_font_size().value,
            });
        }
    }
    Ok(out)
}

pub fn set_text(
    pdfium: &Pdfium,
    path: &Path,
    page_index: u16,
    object_index: usize,
    text: &str,
) -> anyhow::Result<()> {
    with_document(pdfium, path, |doc| {
        let mut page = doc.pages().get(page_index)?;
        let mut object = page
            .objects()
            .get(object_index)
            .map_err(|_| anyhow::anyhow!("object index {object_index} out of range"))?;
        match object.as_text_object_mut() {
            Some(text_obj) => text_obj.set_text(text)?,
            None => anyhow::bail!("object {object_index} is not a text object"),
        }
        page.regenerate_content()?;
        Ok(())
    })
}

pub fn delete_object(
    pdfium: &Pdfium,
    path: &Path,
    page_index: u16,
    object_index: usize,
) -> anyhow::Result<()> {
    with_document(pdfium, path, |doc| {
        let mut page = doc.pages().get(page_index)?;
        let object = page
            .objects()
            .get(object_index)
            .map_err(|_| anyhow::anyhow!("object index {object_index} out of range"))?;
        // The wrapper returned by remove_object destroys the underlying
        // PDFium object on drop, which crashes the process (double free —
        // the page's content regeneration already invalidated the handle).
        // Leaking the small wrapper sidesteps the crash; the object data is
        // gone from the page either way.
        let removed = page.objects_mut().remove_object(object)?;
        std::mem::forget(removed);
        Ok(())
    })
}
