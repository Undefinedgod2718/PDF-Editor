//! Image operations: list / insert / replace the image objects on a page.
//! Object `index` follows the same convention as `objects.rs`: the object's
//! position in the page's full object collection (all object kinds), so
//! indices stay stable across list and mutation endpoints as long as the
//! page isn't otherwise modified.

use std::path::Path;

use image::DynamicImage;
use pdfium_render::prelude::*;
use serde::Serialize;

use super::with_document;

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ImageObjectInfo {
    pub index: usize,
    /// Bounding box in points, top-left origin.
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
    /// Intrinsic pixel dimensions of the embedded image, if PDFium can
    /// report them (0 when unavailable).
    pub px_width: i32,
    pub px_height: i32,
    /// Stream filter names, e.g. ["DCTDecode"]; empty if none reported.
    pub filters: Vec<String>,
    pub bits_per_pixel: Option<u8>,
}

pub fn list_images(doc: &PdfDocument, page_index: u16) -> anyhow::Result<Vec<ImageObjectInfo>> {
    let page = doc.pages().get(page_index)?;
    let page_height = page.height().value;
    let mut out = Vec::new();
    for (index, object) in page.objects().iter().enumerate() {
        if let Some(image_obj) = object.as_image_object() {
            let bounds = object.bounds()?;
            out.push(ImageObjectInfo {
                index,
                x: bounds.left().value,
                y: page_height - bounds.top().value,
                w: bounds.right().value - bounds.left().value,
                h: bounds.top().value - bounds.bottom().value,
                px_width: image_obj.width().unwrap_or(0),
                px_height: image_obj.height().unwrap_or(0),
                filters: image_obj
                    .filters()
                    .iter()
                    .map(|f| f.name().to_string())
                    .collect(),
                bits_per_pixel: image_obj.bits_per_pixel().ok(),
            });
        }
    }
    Ok(out)
}

/// Insert `img` on the page at the given view-space rect (points, top-left
/// origin). PDFium handles the XObject, resource entry, and `cm` matrix.
pub fn insert_image(
    pdfium: &Pdfium,
    path: &Path,
    page_index: u16,
    img: &DynamicImage,
    x: f32,
    y: f32,
    w: f32,
    h: f32,
) -> anyhow::Result<()> {
    anyhow::ensure!(w > 0.0 && h > 0.0, "image width/height must be positive");
    with_document(pdfium, path, |doc| {
        let mut page = doc.pages().get(page_index)?;
        let page_height = page.height().value;
        // View space is top-left origin; PDF user space is bottom-left.
        let pdf_y = page_height - y - h;
        page.objects_mut().create_image_object(
            PdfPoints::new(x),
            PdfPoints::new(pdf_y),
            img,
            Some(PdfPoints::new(w)),
            Some(PdfPoints::new(h)),
        )?;
        page.regenerate_content()?;
        Ok(())
    })
}

/// Swap the bitmap of the image object at `object_index`, keeping its
/// placement matrix. A new image with a different aspect ratio is stretched
/// into the existing box.
pub fn replace_image(
    pdfium: &Pdfium,
    path: &Path,
    page_index: u16,
    object_index: usize,
    img: &DynamicImage,
) -> anyhow::Result<()> {
    with_document(pdfium, path, |doc| {
        let mut page = doc.pages().get(page_index)?;
        let mut object = page
            .objects()
            .get(object_index)
            .map_err(|_| anyhow::anyhow!("object index {object_index} out of range"))?;
        match object.as_image_object_mut() {
            Some(image_obj) => image_obj.set_image(img)?,
            None => anyhow::bail!("object {object_index} is not an image object"),
        }
        page.regenerate_content()?;
        Ok(())
    })
}
