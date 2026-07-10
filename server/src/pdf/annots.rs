//! Annotation create / list / delete. All coordinates crossing the API
//! boundary are PDF points with a top-left origin; PDFium uses a
//! bottom-left origin, so every rect and point is flipped here.
//!
//! PDFium rendering constraints shape two implementations:
//! - Ink strokes are stored as path objects in the annotation's appearance
//!   stream (PDFium only allows object append on Ink and Stamp subtypes).
//! - Text boxes are Stamp annotations carrying a text object, because
//!   PDFium never generates an appearance stream for FreeText, which would
//!   make it invisible in our renderer.
//! - Sticky notes (Text subtype) have no PDFium-generated appearance either;
//!   the frontend draws their icon as an overlay from the list endpoint.

use std::path::Path;

use pdfium_render::prelude::*;
use serde::{Deserialize, Serialize};

#[derive(Deserialize, Clone, Copy)]
pub struct InRect {
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
}

#[derive(Deserialize, Clone, Copy)]
pub struct InPoint {
    pub x: f32,
    pub y: f32,
}

#[derive(Deserialize, Clone, Copy)]
pub struct InColor {
    pub r: u8,
    pub g: u8,
    pub b: u8,
    #[serde(default = "opaque")]
    pub a: u8,
}

fn opaque() -> u8 {
    255
}

impl InColor {
    fn to_pdf(self) -> PdfColor {
        PdfColor::new(self.r, self.g, self.b, self.a)
    }
}

#[derive(Deserialize)]
#[serde(tag = "type", rename_all = "camelCase")]
pub enum NewAnnotation {
    Highlight {
        rects: Vec<InRect>,
        color: InColor,
        #[serde(default)]
        contents: Option<String>,
    },
    Underline {
        rects: Vec<InRect>,
        color: InColor,
        #[serde(default)]
        contents: Option<String>,
    },
    Strikeout {
        rects: Vec<InRect>,
        color: InColor,
        #[serde(default)]
        contents: Option<String>,
    },
    Squiggly {
        rects: Vec<InRect>,
        color: InColor,
        #[serde(default)]
        contents: Option<String>,
    },
    /// Sticky note.
    Note {
        x: f32,
        y: f32,
        contents: String,
        color: InColor,
    },
    /// Freehand drawing; one annotation may hold several strokes.
    Ink {
        strokes: Vec<Vec<InPoint>>,
        color: InColor,
        width: f32,
    },
    /// Text box drawn directly on the page (stored as a Stamp annotation,
    /// see module docs).
    FreeText {
        rect: InRect,
        contents: String,
        color: InColor,
        #[serde(default = "default_font_size")]
        font_size: f32,
    },
}

fn default_font_size() -> f32 {
    12.0
}

#[derive(Serialize)]
pub struct AnnotationInfo {
    pub index: usize,
    #[serde(rename = "type")]
    pub kind: String,
    pub rect: Option<OutRect>,
    pub contents: Option<String>,
}

#[derive(Serialize)]
pub struct OutRect {
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
}

const NOTE_SIZE: f32 = 20.0;

/// Convert a top-left-origin rect to a PDFium bottom-left-origin PdfRect.
fn to_pdf_rect(r: &InRect, page_height: f32) -> PdfRect {
    PdfRect::new_from_values(page_height - (r.y + r.h), r.x, page_height - r.y, r.x + r.w)
}

fn from_pdf_rect(r: &PdfRect, page_height: f32) -> OutRect {
    OutRect {
        x: r.left().value,
        y: page_height - r.top().value,
        w: r.right().value - r.left().value,
        h: r.top().value - r.bottom().value,
    }
}

fn union_rects(rects: &[InRect]) -> InRect {
    let mut min_x = f32::MAX;
    let mut min_y = f32::MAX;
    let mut max_x = f32::MIN;
    let mut max_y = f32::MIN;
    for r in rects {
        min_x = min_x.min(r.x);
        min_y = min_y.min(r.y);
        max_x = max_x.max(r.x + r.w);
        max_y = max_y.max(r.y + r.h);
    }
    InRect {
        x: min_x,
        y: min_y,
        w: max_x - min_x,
        h: max_y - min_y,
    }
}

/// Load, mutate, and atomically save the document back to `path`.
/// The document is loaded from an owned byte buffer so no file handle
/// is held while we overwrite the file.
fn with_document<T>(
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

/// Quad points + bounds + colour + optional popup text, shared by the four
/// text-markup subtypes. A macro because the concrete annotation types share
/// no public trait exposing `attachment_points_mut`.
macro_rules! setup_markup {
    ($annot:expr, $rects:expr, $color:expr, $contents:expr, $page_height:expr) => {{
        if $rects.is_empty() {
            anyhow::bail!("markup annotation needs at least one rect");
        }
        for r in $rects.iter() {
            // PDF spec quad point order: upper-left, upper-right,
            // lower-left, lower-right. PdfQuadPoints::from_rect emits
            // BL,BR,TR,TL, which PDFium's appearance-stream generator
            // rejects, so the quad is built by hand.
            let rect = to_pdf_rect(r, $page_height);
            let quad = PdfQuadPoints::new(
                rect.left(),
                rect.top(),
                rect.right(),
                rect.top(),
                rect.left(),
                rect.bottom(),
                rect.right(),
                rect.bottom(),
            );
            $annot
                .attachment_points_mut()
                .create_attachment_point_at_end(quad)?;
        }
        $annot.set_bounds(to_pdf_rect(&union_rects($rects), $page_height))?;
        $annot.set_stroke_color($color.to_pdf())?;
        if let Some(c) = $contents {
            $annot.set_contents(c)?;
        }
    }};
}

pub fn create(
    pdfium: &Pdfium,
    path: &Path,
    page_index: u16,
    ann: &NewAnnotation,
) -> anyhow::Result<usize> {
    with_document(pdfium, path, |doc| {
        // Font token must be taken before the page borrows the document.
        let font = doc.fonts_mut().helvetica();
        let mut page = doc.pages().get(page_index)?;
        let page_height = page.height().value;
        let annotations = page.annotations_mut();

        match ann {
            NewAnnotation::Highlight { rects, color, contents } => {
                let mut a = annotations.create_highlight_annotation()?;
                setup_markup!(a, rects, color, contents, page_height);
            }
            NewAnnotation::Underline { rects, color, contents } => {
                let mut a = annotations.create_underline_annotation()?;
                setup_markup!(a, rects, color, contents, page_height);
            }
            NewAnnotation::Strikeout { rects, color, contents } => {
                let mut a = annotations.create_strikeout_annotation()?;
                setup_markup!(a, rects, color, contents, page_height);
            }
            NewAnnotation::Squiggly { rects, color, contents } => {
                let mut a = annotations.create_squiggly_annotation()?;
                setup_markup!(a, rects, color, contents, page_height);
            }
            NewAnnotation::Note { x, y, contents, color } => {
                let mut a = annotations.create_text_annotation(contents)?;
                let rect = InRect {
                    x: *x,
                    y: *y,
                    w: NOTE_SIZE,
                    h: NOTE_SIZE,
                };
                a.set_bounds(to_pdf_rect(&rect, page_height))?;
                a.set_stroke_color(color.to_pdf())?;
            }
            NewAnnotation::Ink { strokes, color, width } => {
                if strokes.iter().all(|s| s.len() < 2) {
                    anyhow::bail!("ink annotation needs at least one stroke with 2+ points");
                }
                let mut a = annotations.create_ink_annotation()?;
                let margin = width.max(1.0);
                let all: Vec<InRect> = strokes
                    .iter()
                    .flatten()
                    .map(|p| InRect {
                        x: p.x - margin,
                        y: p.y - margin,
                        w: margin * 2.0,
                        h: margin * 2.0,
                    })
                    .collect();
                a.set_bounds(to_pdf_rect(&union_rects(&all), page_height))?;
                a.set_stroke_color(color.to_pdf())?;
                for stroke in strokes.iter().filter(|s| s.len() >= 2) {
                    let mut path = PdfPagePathObject::new(
                        doc,
                        PdfPoints::new(stroke[0].x),
                        PdfPoints::new(page_height - stroke[0].y),
                        Some(color.to_pdf()),
                        Some(PdfPoints::new(*width)),
                        None,
                    )?;
                    for p in &stroke[1..] {
                        path.line_to(PdfPoints::new(p.x), PdfPoints::new(page_height - p.y))?;
                    }
                    a.objects_mut().add_path_object(path)?;
                }
            }
            NewAnnotation::FreeText { rect, contents, color, font_size } => {
                // Stamp subtype: the only annotation type (besides Ink) that
                // accepts appended page objects, giving us a renderable
                // appearance stream. See module docs.
                let mut a = annotations.create_stamp_annotation()?;
                a.set_bounds(to_pdf_rect(rect, page_height))?;
                a.set_contents(contents)?;
                let mut line_y = rect.y + font_size;
                for line in contents.lines() {
                    if line_y > rect.y + rect.h {
                        break;
                    }
                    let mut text_obj = PdfPageTextObject::new(
                        doc,
                        line,
                        font,
                        PdfPoints::new(*font_size),
                    )?;
                    text_obj.set_fill_color(color.to_pdf())?;
                    text_obj.translate(
                        PdfPoints::new(rect.x + 2.0),
                        PdfPoints::new(page_height - line_y),
                    )?;
                    a.objects_mut().add_text_object(text_obj)?;
                    line_y += font_size * 1.3;
                }
            }
        }
        Ok(annotations.len() as usize)
    })
}

pub fn list(pdfium: &Pdfium, path: &Path, page_index: u16) -> anyhow::Result<Vec<AnnotationInfo>> {
    let doc = pdfium.load_pdf_from_file(path, None)?;
    let page = doc.pages().get(page_index)?;
    let page_height = page.height().value;
    let mut out = Vec::new();
    for (index, annot) in page.annotations().iter().enumerate() {
        out.push(AnnotationInfo {
            index,
            kind: format!("{:?}", annot.annotation_type()),
            rect: annot.bounds().ok().map(|b| from_pdf_rect(&b, page_height)),
            contents: annot.contents(),
        });
    }
    Ok(out)
}

pub fn delete(pdfium: &Pdfium, path: &Path, page_index: u16, index: usize) -> anyhow::Result<()> {
    with_document(pdfium, path, |doc| {
        let mut page = doc.pages().get(page_index)?;
        let annot = page
            .annotations()
            .get(index as _)
            .map_err(|_| anyhow::anyhow!("annotation index {index} out of range"))?;
        page.annotations_mut().delete_annotation(annot)?;
        Ok(())
    })
}
