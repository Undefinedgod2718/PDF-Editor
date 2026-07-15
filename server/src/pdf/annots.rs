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
    #[serde(rename_all = "camelCase")]
    FreeText {
        rect: InRect,
        contents: String,
        color: InColor,
        #[serde(default = "default_font_size")]
        font_size: f32,
    },
    /// Image stamp from the stamp library; the PNG (alpha preserved) is
    /// resolved by the API layer and passed to [create] separately.
    #[serde(rename_all = "camelCase")]
    Stamp {
        rect: InRect,
        stamp_id: uuid::Uuid,
    },
}

fn default_font_size() -> f32 {
    12.0
}

#[derive(Serialize)]
pub struct AnnotationInfo {
    pub index: usize,
    /// Stable id from the annotation's /NM entry (PDF 32000-1 12.5.2).
    /// Preferred over `index` for deletion: indices shift when an earlier
    /// annotation on the page is removed. `None` only for annotations
    /// created before /NM stamping was introduced.
    pub nm: Option<String>,
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

use super::with_document;

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
    stamp_image: Option<image::DynamicImage>,
) -> anyhow::Result<usize> {
    with_document(pdfium, path, |doc| {
        // Font token must be taken before the page borrows the document.
        // CJK-capable font preferred (GenSen Rounded TW, SIL OFL), subset to
        // just the glyphs this annotation uses so the file stays small; fall
        // back to built-in Helvetica (ASCII only) when unavailable.
        let font = match ann {
            NewAnnotation::FreeText { contents, .. } => super::font::full_font_bytes()
                .and_then(|full| {
                    super::font::subset_for_text(full, contents)
                        .map_err(|e| tracing::warn!("font subset failed: {e}"))
                        .ok()
                })
                .and_then(|subset| {
                    doc.fonts_mut()
                        .load_true_type_from_bytes(&subset, true)
                        .map_err(|e| tracing::warn!("subset font load failed: {e:?}"))
                        .ok()
                })
                .unwrap_or_else(|| doc.fonts_mut().helvetica()),
            _ => doc.fonts_mut().helvetica(),
        };
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
            NewAnnotation::Stamp { rect, stamp_id } => {
                let img = stamp_image
                    .ok_or_else(|| anyhow::anyhow!("stamp image {stamp_id} not resolved"))?;
                let mut a = annotations.create_stamp_annotation()?;
                a.set_bounds(to_pdf_rect(rect, page_height))?;
                let mut obj = PdfPageImageObject::new(doc, &img)?;
                // Image objects start out 1x1 pt; scale to the target rect,
                // then move so the image's bottom-left sits at the rect's
                // bottom-left in PDF coordinates.
                obj.scale(rect.w, rect.h)?;
                obj.translate(
                    PdfPoints::new(rect.x),
                    PdfPoints::new(page_height - (rect.y + rect.h)),
                )?;
                a.objects_mut().add_image_object(obj)?;
            }
        }
        Ok(annotations.len() as usize)
    })
}

pub fn list(doc: &PdfDocument, page_index: u16) -> anyhow::Result<Vec<AnnotationInfo>> {
    let page = doc.pages().get(page_index)?;
    let page_height = page.height().value;
    let mut out = Vec::new();
    for (index, annot) in page.annotations().iter().enumerate() {
        out.push(AnnotationInfo {
            index,
            nm: annot.name(),
            kind: format!("{:?}", annot.annotation_type()),
            rect: annot.bounds().ok().map(|b| from_pdf_rect(&b, page_height)),
            contents: annot.contents(),
        });
    }
    Ok(out)
}

/// Delete by the annotation's stable /NM name. A purely numeric `annot_id`
/// additionally falls back to positional index, but only when the annotation
/// at that index has no /NM — a pre-/NM leftover the client could not address
/// any other way. An indexed hit that *does* carry /NM is rejected instead:
/// the client is holding a stale list and the index may now point at a
/// different annotation (the original index-shift bug this replaces).
pub fn delete(pdfium: &Pdfium, path: &Path, page_index: u16, annot_id: &str) -> anyhow::Result<()> {
    with_document(pdfium, path, |doc| {
        let mut page = doc.pages().get(page_index)?;
        let target = {
            let annots = page.annotations();
            let by_name = annots
                .iter()
                .position(|a| a.name().as_deref() == Some(annot_id));
            match (by_name, annot_id.parse::<usize>()) {
                (Some(i), _) => i,
                (None, Ok(index)) => {
                    let annot = annots
                        .get(index as _)
                        .map_err(|_| anyhow::anyhow!("annotation index {index} out of range"))?;
                    if annot.name().is_some() {
                        anyhow::bail!(
                            "annotation at index {index} has a stable id; refresh the list and delete by id"
                        );
                    }
                    index
                }
                (None, Err(_)) => anyhow::bail!("annotation {annot_id} not found"),
            }
        };
        let annot = page
            .annotations()
            .get(target as _)
            .map_err(|_| anyhow::anyhow!("annotation index {target} out of range"))?;
        page.annotations_mut().delete_annotation(annot)?;
        Ok(())
    })
}

/// Stamp a stable /NM name (UUID) onto every user annotation that lacks one.
///
/// pdfium-render 0.8 can read /NM (`PdfPageAnnotationCommon::name`) but has
/// no public writer (`set_string_value` is crate-private), so this runs as a
/// lopdf pass after annotation writes — same dictionary-surgery approach as
/// `formops::set_button_lopdf`. Sweeping the whole document also back-fills
/// annotations created before /NM stamping existed.
///
/// Widget, Link, and Popup annotations are left untouched: widgets are form
/// machinery addressed by index in `formops`, and links/popups are not
/// user-managed annotations.
pub fn ensure_annotation_names(path: &Path) -> anyhow::Result<()> {
    use lopdf::{Dictionary, Document, Object, ObjectId};

    // Caller normally ran `with_document` first; keep the guard for any
    // future direct call — a lopdf load+save also strips /Encrypt.
    super::protect::assert_editable(path)?;

    fn needs_name(dict: &Dictionary) -> bool {
        let subtype = dict
            .get(b"Subtype")
            .ok()
            .and_then(|s| s.as_name().ok())
            .unwrap_or_default();
        !matches!(subtype, b"Widget" | b"Link" | b"Popup") && !dict.has(b"NM")
    }

    fn stamp(dict: &mut Dictionary) {
        dict.set(
            "NM",
            Object::string_literal(uuid::Uuid::new_v4().to_string()),
        );
    }

    /// PDFium's save writes annotations as inline dictionaries inside the
    /// /Annots array; other producers use indirect references. Handle both:
    /// stamp inline dicts in place, collect references for a later pass.
    fn stamp_array(arr: &mut [Object], refs: &mut Vec<ObjectId>, dirty: &mut bool) {
        for entry in arr.iter_mut() {
            match entry {
                Object::Dictionary(d) if needs_name(d) => {
                    stamp(d);
                    *dirty = true;
                }
                Object::Reference(r) => refs.push(*r),
                _ => {}
            }
        }
    }

    let mut doc = Document::load(path)?;
    let page_ids: Vec<ObjectId> = doc.get_pages().values().copied().collect();
    let mut dirty = false;
    let mut annot_refs: Vec<ObjectId> = Vec::new();

    for page_id in page_ids {
        // /Annots itself may also be direct or a reference to an array object.
        let mut array_ref = None;
        if let Ok(page) = doc.get_dictionary_mut(page_id) {
            match page.get_mut(b"Annots") {
                Ok(Object::Array(arr)) => stamp_array(arr, &mut annot_refs, &mut dirty),
                Ok(Object::Reference(r)) => array_ref = Some(*r),
                _ => {}
            }
        }
        if let Some(rid) = array_ref {
            if let Ok(Object::Array(arr)) = doc.get_object_mut(rid) {
                stamp_array(arr, &mut annot_refs, &mut dirty);
            }
        }
    }

    for rid in annot_refs {
        if let Ok(dict) = doc.get_dictionary_mut(rid) {
            if needs_name(dict) {
                stamp(dict);
                dirty = true;
            }
        }
    }

    if !dirty {
        return Ok(());
    }
    let mut bytes = Vec::new();
    doc.save_to(&mut bytes)?;
    let tmp = path.with_extension("pdf.tmp");
    std::fs::write(&tmp, &bytes)?;
    std::fs::rename(&tmp, path)?;
    Ok(())
}
