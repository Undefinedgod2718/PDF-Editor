//! P13 — compare two documents: a per-page text diff (`similar` crate, char
//! granularity so spans map directly back onto `CharBox` rects) plus a
//! coarse tile-based pixel diff, both burned into a copy of the "new"
//! document as highlight/note annotations via `annots::create_on_doc`.
//!
//! Page alignment is index-based (v1 scope, see wiki/P13-Review.md): pages
//! beyond the shorter document's page count are treated as pure
//! insertions/deletions rather than content-matched. Visual diff only runs
//! on page pairs where a text change was already detected, since a page
//! with no text difference is overwhelmingly likely to render identically
//! and re-rendering every page of a large document at full cost is wasted
//! work.
//!
//! Deleted text spans on a page that still exists in both documents (a
//! "modified" page, as opposed to a wholly removed one) have no rect to
//! highlight in the output — that page's content only survives in `old`,
//! and the output document is built from `new`. Those are reported as an
//! aggregated corner note instead of per-span highlights; the full detail
//! is still in the JSON report and LLM summary.

use std::path::Path;

use image::RgbaImage;
use pdfium_render::prelude::*;
use serde::Serialize;
use similar::{ChangeTag, TextDiff};

use super::annots::{self, InColor, InRect, NewAnnotation};
use super::ops::{self, CharBox, PageText, Rect};

#[derive(Serialize, Clone, Copy, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum ChangeKind {
    Added,
    Deleted,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TextChange {
    pub kind: ChangeKind,
    pub rects: Vec<Rect>,
    pub excerpt: String,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PageDiff {
    pub old_page: Option<u16>,
    pub new_page: Option<u16>,
    pub text_changes: Vec<TextChange>,
    pub visual_changed: bool,
    pub visual_regions: Vec<Rect>,
}

#[derive(Serialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct CompareStats {
    pub pages_added: u16,
    pub pages_deleted: u16,
    pub pages_modified: u16,
    pub text_changes_total: usize,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CompareReport {
    pub old_page_count: u16,
    pub new_page_count: u16,
    pub pages: Vec<PageDiff>,
    pub stats: CompareStats,
    /// Filled in by the API handler after this report is produced — an LLM
    /// call is not PDFium work and must not run on the PDFium worker thread.
    pub summary: Option<String>,
}

pub struct CompareOptions {
    pub visual_diff: bool,
}

const RENDER_SCALE: f32 = 1.5;
const EXCERPT_CONTEXT: usize = 20;
const TILE_SIZE: u32 = 32;
/// Average per-pixel RGB delta (0..765) above which a tile counts as changed.
const TILE_THRESHOLD: u32 = 24;

const COLOR_ADDED: InColor = InColor { r: 76, g: 175, b: 80, a: 80 };
const COLOR_DELETED: InColor = InColor { r: 244, g: 67, b: 54, a: 80 };
const COLOR_VISUAL: InColor = InColor { r: 33, g: 150, b: 243, a: 60 };

/// Pair up old/new page indices. `None` on one side means that page has no
/// counterpart in the other document (pure insertion/deletion). Index-based
/// only — see module docs for the tradeoff.
pub fn align_pages(old_count: u16, new_count: u16) -> Vec<(Option<u16>, Option<u16>)> {
    let min = old_count.min(new_count);
    let mut pairs: Vec<(Option<u16>, Option<u16>)> =
        (0..min).map(|i| (Some(i), Some(i))).collect();
    for i in min..old_count {
        pairs.push((Some(i), None));
    }
    for i in min..new_count {
        pairs.push((None, Some(i)));
    }
    pairs
}

fn to_in_rect(r: &Rect) -> InRect {
    InRect { x: r.x, y: r.y, w: r.w, h: r.h }
}

fn excerpt_around(chars: &[CharBox], start: usize, end_exclusive: usize) -> String {
    let from = start.saturating_sub(EXCERPT_CONTEXT);
    let to = (end_exclusive + EXCERPT_CONTEXT).min(chars.len());
    chars[from..to].iter().map(|cb| cb.c).collect()
}

/// Char-level diff between two pages' text layers, grouped into contiguous
/// added/deleted spans. `similar`'s char tokenization matches `PageText`'s
/// own `.chars()` iteration, so a change's `old_index`/`new_index` lines up
/// directly with `PageText.chars`.
pub fn diff_page_text(old: &PageText, new: &PageText) -> Vec<TextChange> {
    let diff = TextDiff::from_chars(old.text.as_str(), new.text.as_str());
    let mut changes = Vec::new();
    let mut current: Option<(ChangeTag, usize, usize)> = None;

    fn flush(
        current: &mut Option<(ChangeTag, usize, usize)>,
        changes: &mut Vec<TextChange>,
        old: &PageText,
        new: &PageText,
    ) {
        let Some((tag, start, end)) = current.take() else {
            return;
        };
        let (kind, chars) = match tag {
            ChangeTag::Delete => (ChangeKind::Deleted, &old.chars),
            ChangeTag::Insert => (ChangeKind::Added, &new.chars),
            ChangeTag::Equal => return,
        };
        let rects = ops::merge_rects(chars[start..=end].iter().map(|cb| cb.rect));
        let excerpt = excerpt_around(chars, start, end + 1);
        changes.push(TextChange { kind, rects, excerpt });
    }

    for change in diff.iter_all_changes() {
        match change.tag() {
            ChangeTag::Equal => flush(&mut current, &mut changes, old, new),
            tag => {
                let idx = match tag {
                    ChangeTag::Delete => change.old_index(),
                    ChangeTag::Insert => change.new_index(),
                    ChangeTag::Equal => unreachable!(),
                };
                let Some(idx) = idx else { continue };
                match &mut current {
                    Some((cur_tag, _, end)) if *cur_tag == tag && *end + 1 == idx => {
                        *end = idx;
                    }
                    _ => {
                        flush(&mut current, &mut changes, old, new);
                        current = Some((tag, idx, idx));
                    }
                }
            }
        }
    }
    flush(&mut current, &mut changes, old, new);
    changes
}

/// Coarse tile-based pixel diff. Requires identical rendered dimensions
/// (different page sizes are skipped, not rescaled — out of v1 scope).
/// Returns whether anything changed and merged bounding boxes of the
/// changed regions, in PDF points.
pub fn diff_page_visual(old_img: &RgbaImage, new_img: &RgbaImage, scale: f32) -> (bool, Vec<Rect>) {
    if old_img.dimensions() != new_img.dimensions() {
        return (false, Vec::new());
    }
    let (w, h) = old_img.dimensions();
    let tiles_x = w.div_ceil(TILE_SIZE);
    let tiles_y = h.div_ceil(TILE_SIZE);
    let mut changed_tiles = Vec::new();

    for ty in 0..tiles_y {
        for tx in 0..tiles_x {
            let x0 = tx * TILE_SIZE;
            let y0 = ty * TILE_SIZE;
            let x1 = (x0 + TILE_SIZE).min(w);
            let y1 = (y0 + TILE_SIZE).min(h);
            let mut sum: u64 = 0;
            let mut count: u64 = 0;
            for y in y0..y1 {
                for x in x0..x1 {
                    let a = old_img.get_pixel(x, y);
                    let b = new_img.get_pixel(x, y);
                    let d = (a[0] as i32 - b[0] as i32).unsigned_abs()
                        + (a[1] as i32 - b[1] as i32).unsigned_abs()
                        + (a[2] as i32 - b[2] as i32).unsigned_abs();
                    sum += d as u64;
                    count += 1;
                }
            }
            if count > 0 && (sum / count) as u32 > TILE_THRESHOLD {
                changed_tiles.push((tx, ty));
            }
        }
    }

    if changed_tiles.is_empty() {
        return (false, Vec::new());
    }
    (true, merge_tile_rects(&changed_tiles, scale))
}

/// Flood-fill 4-connected changed tiles into bounding-box rects, in points.
fn merge_tile_rects(tiles: &[(u32, u32)], scale: f32) -> Vec<Rect> {
    use std::collections::HashSet;
    let set: HashSet<(u32, u32)> = tiles.iter().copied().collect();
    let mut visited: HashSet<(u32, u32)> = HashSet::new();
    let mut out = Vec::new();

    for &start in tiles {
        if visited.contains(&start) {
            continue;
        }
        let mut stack = vec![start];
        visited.insert(start);
        let (mut min_x, mut max_x) = (start.0, start.0);
        let (mut min_y, mut max_y) = (start.1, start.1);
        while let Some((x, y)) = stack.pop() {
            min_x = min_x.min(x);
            max_x = max_x.max(x);
            min_y = min_y.min(y);
            max_y = max_y.max(y);
            let neighbors = [
                (x as i64 + 1, y as i64),
                (x as i64 - 1, y as i64),
                (x as i64, y as i64 + 1),
                (x as i64, y as i64 - 1),
            ];
            for (nx, ny) in neighbors {
                if nx < 0 || ny < 0 {
                    continue;
                }
                let n = (nx as u32, ny as u32);
                if set.contains(&n) && visited.insert(n) {
                    stack.push(n);
                }
            }
        }
        let px_x0 = (min_x * TILE_SIZE) as f32;
        let px_y0 = (min_y * TILE_SIZE) as f32;
        let px_x1 = ((max_x + 1) * TILE_SIZE) as f32;
        let px_y1 = ((max_y + 1) * TILE_SIZE) as f32;
        out.push(Rect {
            x: px_x0 / scale,
            y: px_y0 / scale,
            w: (px_x1 - px_x0) / scale,
            h: (px_y1 - px_y0) / scale,
        });
    }
    out
}

/// Burn the diff for one aligned page pair into `dest` (already holding the
/// full "new" document plus any appended old-only pages).
fn annotate_page_diff(
    dest: &mut PdfDocument,
    page: &PageDiff,
    old_only_dest_index: &[(u16, u16)],
) -> anyhow::Result<()> {
    match (page.old_page, page.new_page) {
        (Some(_), Some(new_idx)) => {
            let added_rects: Vec<InRect> = page
                .text_changes
                .iter()
                .filter(|c| c.kind == ChangeKind::Added)
                .flat_map(|c| c.rects.iter().map(to_in_rect))
                .collect();
            if !added_rects.is_empty() {
                annots::create_on_doc(
                    dest,
                    new_idx,
                    &NewAnnotation::Highlight {
                        rects: added_rects,
                        color: COLOR_ADDED,
                        contents: Some("新增內容".into()),
                    },
                    None,
                )?;
            }

            let deleted_excerpts: Vec<&str> = page
                .text_changes
                .iter()
                .filter(|c| c.kind == ChangeKind::Deleted)
                .map(|c| c.excerpt.as_str())
                .collect();
            if !deleted_excerpts.is_empty() {
                let width = dest.pages().get(new_idx)?.width().value;
                annots::create_on_doc(
                    dest,
                    new_idx,
                    &NewAnnotation::Note {
                        x: width - 30.0,
                        y: 10.0,
                        contents: format!("已刪除內容：{}", deleted_excerpts.join(" / ")),
                        color: COLOR_DELETED,
                    },
                    None,
                )?;
            }

            if page.visual_changed && !page.visual_regions.is_empty() {
                let rects: Vec<InRect> = page.visual_regions.iter().map(to_in_rect).collect();
                annots::create_on_doc(
                    dest,
                    new_idx,
                    &NewAnnotation::Highlight {
                        rects,
                        color: COLOR_VISUAL,
                        contents: Some("偵測到視覺差異".into()),
                    },
                    None,
                )?;
            }
        }
        (None, Some(new_idx)) => {
            let width = dest.pages().get(new_idx)?.width().value;
            annots::create_on_doc(
                dest,
                new_idx,
                &NewAnnotation::Note {
                    x: width - 30.0,
                    y: 10.0,
                    contents: "此頁為新增頁面".into(),
                    color: COLOR_ADDED,
                },
                None,
            )?;
        }
        (Some(old_idx), None) => {
            let dest_idx = old_only_dest_index
                .iter()
                .find(|&&(oi, _)| oi == old_idx)
                .map(|&(_, di)| di)
                .ok_or_else(|| anyhow::anyhow!("missing dest index for deleted page {old_idx}"))?;
            let width = dest.pages().get(dest_idx)?.width().value;
            annots::create_on_doc(
                dest,
                dest_idx,
                &NewAnnotation::Note {
                    x: width - 30.0,
                    y: 10.0,
                    contents: "此頁已從原文件刪除".into(),
                    color: COLOR_DELETED,
                },
                None,
            )?;
        }
        (None, None) => {}
    }
    Ok(())
}

/// Compare `old_path` against `new_path`, returning the structured report
/// (LLM `summary` left `None` — filled in by the caller, see module docs)
/// and the bytes of a new document: a copy of `new`, with old-only pages
/// appended at the end, annotated throughout with the detected differences.
pub fn compare(
    pdfium: &Pdfium,
    old_path: &Path,
    new_path: &Path,
    opts: &CompareOptions,
) -> anyhow::Result<(CompareReport, Vec<u8>)> {
    let old_bytes = std::fs::read(old_path)?;
    let new_bytes = std::fs::read(new_path)?;
    let old_doc = pdfium.load_pdf_from_byte_vec(old_bytes, None)?;
    let new_doc = pdfium.load_pdf_from_byte_vec(new_bytes, None)?;
    let old_count = old_doc.pages().len();
    let new_count = new_doc.pages().len();

    let mut dest = pdfium.create_new_pdf()?;
    dest.pages_mut().append(&new_doc)?;

    let alignment = align_pages(old_count, new_count);
    let mut pages = Vec::with_capacity(alignment.len());
    let mut stats = CompareStats::default();
    let mut old_only_dest_index: Vec<(u16, u16)> = Vec::new();
    let mut next_dest_index = new_count;

    for &(old_idx, new_idx) in &alignment {
        match (old_idx, new_idx) {
            (Some(oi), Some(ni)) => {
                let old_text = ops::page_text(&old_doc, oi)?;
                let new_text = ops::page_text(&new_doc, ni)?;
                let text_changes = diff_page_text(&old_text, &new_text);

                let (visual_changed, visual_regions) = if opts.visual_diff && !text_changes.is_empty()
                {
                    let old_img = ops::render_page_image(&old_doc, oi, RENDER_SCALE)?;
                    let new_img = ops::render_page_image(&new_doc, ni, RENDER_SCALE)?;
                    diff_page_visual(&old_img, &new_img, RENDER_SCALE)
                } else {
                    (false, Vec::new())
                };

                if !text_changes.is_empty() || visual_changed {
                    stats.pages_modified += 1;
                }
                stats.text_changes_total += text_changes.len();

                pages.push(PageDiff {
                    old_page: Some(oi),
                    new_page: Some(ni),
                    text_changes,
                    visual_changed,
                    visual_regions,
                });
            }
            (None, Some(ni)) => {
                stats.pages_added += 1;
                pages.push(PageDiff {
                    old_page: None,
                    new_page: Some(ni),
                    text_changes: Vec::new(),
                    visual_changed: false,
                    visual_regions: Vec::new(),
                });
            }
            (Some(oi), None) => {
                stats.pages_deleted += 1;
                old_only_dest_index.push((oi, next_dest_index));
                next_dest_index += 1;
                pages.push(PageDiff {
                    old_page: Some(oi),
                    new_page: None,
                    text_changes: Vec::new(),
                    visual_changed: false,
                    visual_regions: Vec::new(),
                });
            }
            (None, None) => {}
        }
    }

    for &(old_idx, dest_idx) in &old_only_dest_index {
        dest.pages_mut()
            .copy_page_from_document(&old_doc, old_idx, dest_idx)?;
    }

    for page in &pages {
        annotate_page_diff(&mut dest, page, &old_only_dest_index)?;
    }

    let bytes = dest.save_to_bytes()?;
    let report = CompareReport {
        old_page_count: old_count,
        new_page_count: new_count,
        pages,
        stats,
        summary: None,
    };
    Ok((report, bytes))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn align_equal_length() {
        assert_eq!(
            align_pages(3, 3),
            vec![(Some(0), Some(0)), (Some(1), Some(1)), (Some(2), Some(2))]
        );
    }

    #[test]
    fn align_new_longer() {
        assert_eq!(
            align_pages(2, 4),
            vec![
                (Some(0), Some(0)),
                (Some(1), Some(1)),
                (None, Some(2)),
                (None, Some(3)),
            ]
        );
    }

    #[test]
    fn align_old_longer() {
        assert_eq!(
            align_pages(4, 2),
            vec![
                (Some(0), Some(0)),
                (Some(1), Some(1)),
                (Some(2), None),
                (Some(3), None),
            ]
        );
    }
}
