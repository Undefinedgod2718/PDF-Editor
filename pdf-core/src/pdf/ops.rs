use std::io::Cursor;

use image::{ImageFormat, RgbaImage};
use pdfium_render::prelude::*;
use serde::Serialize;

#[derive(Serialize)]
pub struct PageInfo {
    pub index: u16,
    /// Page width in PDF points.
    pub width: f32,
    /// Page height in PDF points.
    pub height: f32,
    /// Page rotation in degrees (0/90/180/270).
    pub rotation: u16,
}

#[derive(Serialize)]
pub struct DocInfo {
    pub page_count: u16,
    pub title: Option<String>,
    pub pages: Vec<PageInfo>,
}

/// Axis-aligned rectangle in page space: points, origin at top-left.
#[derive(Serialize, Clone, Copy)]
pub struct Rect {
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
}

#[derive(Serialize)]
pub struct CharBox {
    pub c: char,
    #[serde(flatten)]
    pub rect: Rect,
}

#[derive(Serialize)]
pub struct PageText {
    pub text: String,
    pub chars: Vec<CharBox>,
}

#[derive(Serialize)]
pub struct SearchHit {
    pub page: u16,
    pub rects: Vec<Rect>,
    pub excerpt: String,
}

fn to_top_left(bounds: &PdfRect, page_height: f32) -> Rect {
    Rect {
        x: bounds.left().value,
        y: page_height - bounds.top().value,
        w: bounds.right().value - bounds.left().value,
        h: bounds.top().value - bounds.bottom().value,
    }
}

pub fn doc_info(doc: &PdfDocument) -> anyhow::Result<DocInfo> {
    let title = doc.metadata().get(PdfDocumentMetadataTagType::Title).map(|t| t.value().to_string());
    let mut pages = Vec::new();
    for (index, page) in doc.pages().iter().enumerate() {
        let rotation = match page.rotation() {
            Ok(PdfPageRenderRotation::Degrees90) => 90,
            Ok(PdfPageRenderRotation::Degrees180) => 180,
            Ok(PdfPageRenderRotation::Degrees270) => 270,
            _ => 0,
        };
        pages.push(PageInfo {
            index: index as u16,
            width: page.width().value,
            height: page.height().value,
            rotation,
        });
    }
    Ok(DocInfo {
        page_count: doc.pages().len(),
        title,
        pages,
    })
}

/// Render one page to a raw RGBA bitmap. `scale` is pixels per PDF point
/// (1.0 = 72 dpi). Shared by the PNG render endpoint and the pixel-diff
/// pass in `compare.rs`, which needs the buffer without a PNG round-trip.
pub fn render_page_image(doc: &PdfDocument, index: u16, scale: f32) -> anyhow::Result<RgbaImage> {
    let page = doc.pages().get(index)?;
    let width = (page.width().value * scale).round() as i32;
    let config = PdfRenderConfig::new()
        .set_target_width(width)
        .render_form_data(true)
        .render_annotations(true);
    let bitmap = page.render_with_config(&config)?;
    Ok(bitmap.as_image().to_rgba8())
}

/// Render one page to PNG. `scale` is pixels per PDF point (1.0 = 72 dpi).
pub fn render_page(doc: &PdfDocument, index: u16, scale: f32) -> anyhow::Result<Vec<u8>> {
    let image = render_page_image(doc, index, scale)?;
    let mut buf = Cursor::new(Vec::new());
    image.write_to(&mut buf, ImageFormat::Png)?;
    Ok(buf.into_inner())
}

pub fn page_text(doc: &PdfDocument, index: u16) -> anyhow::Result<PageText> {
    let page = doc.pages().get(index)?;
    let page_height = page.height().value;
    let text = page.text()?;
    let mut chars = Vec::new();
    let mut full = String::new();
    for ch in text.chars().iter() {
        let c = ch.unicode_char().unwrap_or(' ');
        full.push(c);
        let bounds = ch.loose_bounds()?;
        chars.push(CharBox {
            c,
            rect: to_top_left(&bounds, page_height),
        });
    }
    Ok(PageText { text: full, chars })
}

/// Case-insensitive substring search across all pages, returning one hit
/// with merged per-character rects per match.
pub fn search(doc: &PdfDocument, query: &str) -> anyhow::Result<Vec<SearchHit>> {
    let needle: Vec<char> = query.to_lowercase().chars().collect();
    if needle.is_empty() {
        return Ok(Vec::new());
    }
    let mut hits = Vec::new();
    for (page_index, page) in doc.pages().iter().enumerate() {
        let page_height = page.height().value;
        let text = page.text()?;
        let boxes: Vec<(char, PdfRect)> = text
            .chars()
            .iter()
            .map(|ch| {
                let c = ch.unicode_char().unwrap_or(' ');
                let b = ch.loose_bounds().unwrap_or(PdfRect::ZERO);
                (c, b)
            })
            .collect();
        let lower: Vec<char> = boxes
            .iter()
            .map(|(c, _)| c.to_lowercase().next().unwrap_or(*c))
            .collect();

        let mut i = 0;
        while i + needle.len() <= lower.len() {
            if lower[i..i + needle.len()] == needle[..] {
                let slice = &boxes[i..i + needle.len()];
                let rects = merge_char_rects(slice, page_height);
                let start = i.saturating_sub(20);
                let end = (i + needle.len() + 20).min(boxes.len());
                let excerpt: String = boxes[start..end].iter().map(|(c, _)| *c).collect();
                hits.push(SearchHit {
                    page: page_index as u16,
                    rects,
                    excerpt,
                });
                i += needle.len();
            } else {
                i += 1;
            }
        }
    }
    Ok(hits)
}

/// Merge adjacent rects (already top-left, points) that sit on the same
/// line into wider rects. Used for per-character search hits and, via
/// `compare.rs`, for diff highlight spans built from `CharBox` slices.
pub(crate) fn merge_rects(rects: impl IntoIterator<Item = Rect>) -> Vec<Rect> {
    let mut out: Vec<Rect> = Vec::new();
    for r in rects {
        if r.w <= 0.0 || r.h <= 0.0 {
            continue;
        }
        if let Some(last) = out.last_mut() {
            let same_line = (last.y - r.y).abs() < last.h * 0.5;
            let adjacent = r.x <= last.x + last.w + last.h; // gap smaller than line height
            if same_line && adjacent {
                let right = (last.x + last.w).max(r.x + r.w);
                let bottom = (last.y + last.h).max(r.y + r.h);
                last.y = last.y.min(r.y);
                last.w = right - last.x;
                last.h = bottom - last.y;
                continue;
            }
        }
        out.push(r);
    }
    out
}

/// Merge adjacent character rects on the same line into wider rects.
fn merge_char_rects(chars: &[(char, PdfRect)], page_height: f32) -> Vec<Rect> {
    merge_rects(chars.iter().map(|(_, b)| to_top_left(b, page_height)))
}
