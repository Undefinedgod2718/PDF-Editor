//! P15 limited text editing: line-level operations on page text objects.
//!
//! A "line" is a cluster of text objects whose baselines sit within half a
//! text height of each other. Line indices are positions in the top-to-bottom
//! ordering returned by [`list_lines`] and are only stable until the page is
//! next modified — clients must re-fetch after every mutation.
//!
//! Scope is deliberately limited (no reflow): editing a line rewrites its
//! anchor object in place, inserting a line places a new text object below
//! the reference line (optionally translating everything underneath down to
//! make room), and shifting translates existing objects vertically.

use std::path::Path;

use pdfium_render::prelude::*;
use serde::{Deserialize, Serialize};

use super::with_document;

#[derive(Serialize)]
pub struct LineInfo {
    /// Position in the top-to-bottom line ordering (unstable across edits).
    pub index: usize,
    pub text: String,
    /// Bounding box in points, top-left origin.
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
    pub font_size: f32,
    /// Fill color of the line's first object, RGBA.
    pub color: [u8; 4],
    /// Page object indices of the text objects forming this line.
    pub objects: Vec<usize>,
}

#[derive(Deserialize)]
pub struct InsertLine {
    /// Line index the new line goes below; its style (size, color, x) is copied.
    pub after: usize,
    pub text: String,
    /// Translate every object below the reference line down by one leading
    /// to make room, instead of overprinting whatever is there.
    #[serde(default)]
    pub shift_down: bool,
}

#[derive(Deserialize)]
pub struct ShiftLine {
    /// Vertical shift in points, positive = down the page.
    pub delta: f32,
    /// Also shift every line below this one by the same amount.
    #[serde(default)]
    pub and_below: bool,
}

/// One text object's geometry in PDF coordinates (origin bottom-left).
struct TextRun {
    object_index: usize,
    text: String,
    left: f32,
    right: f32,
    top: f32,
    bottom: f32,
    font_size: f32,
    color: [u8; 4],
}

struct Line {
    runs: Vec<TextRun>,
}

impl Line {
    fn top(&self) -> f32 {
        self.runs.iter().map(|r| r.top).fold(f32::MIN, f32::max)
    }
    fn bottom(&self) -> f32 {
        self.runs.iter().map(|r| r.bottom).fold(f32::MAX, f32::min)
    }
}

fn collect_runs(page: &PdfPage) -> anyhow::Result<Vec<TextRun>> {
    let mut runs = Vec::new();
    for (object_index, object) in page.objects().iter().enumerate() {
        let Some(text_obj) = object.as_text_object() else {
            continue;
        };
        let bounds = object.bounds()?;
        let color = object
            .fill_color()
            .map(|c| [c.red(), c.green(), c.blue(), c.alpha()])
            .unwrap_or([0, 0, 0, 255]);
        runs.push(TextRun {
            object_index,
            text: text_obj.text(),
            left: bounds.left().value,
            right: bounds.right().value,
            top: bounds.top().value,
            bottom: bounds.bottom().value,
            font_size: text_obj.scaled_font_size().value,
            color,
        });
    }
    Ok(runs)
}

/// Cluster runs into lines by baseline proximity: a run joins a line when its
/// bottom sits within half the smaller text height of the line's bottom.
fn group_lines(mut runs: Vec<TextRun>) -> Vec<Line> {
    // Top of page first (PDF coords: larger y is higher), then left to right.
    runs.sort_by(|a, b| {
        b.bottom
            .partial_cmp(&a.bottom)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then(a.left.partial_cmp(&b.left).unwrap_or(std::cmp::Ordering::Equal))
    });
    let mut lines: Vec<Line> = Vec::new();
    for run in runs {
        let run_h = (run.top - run.bottom).max(1.0);
        let joined = lines.iter_mut().find(|line| {
            let line_h = (line.top() - line.bottom()).max(1.0);
            (line.bottom() - run.bottom).abs() < line_h.min(run_h) * 0.5
        });
        match joined {
            Some(line) => line.runs.push(run),
            None => lines.push(Line { runs: vec![run] }),
        }
    }
    for line in &mut lines {
        line.runs.sort_by(|a, b| {
            a.left.partial_cmp(&b.left).unwrap_or(std::cmp::Ordering::Equal)
        });
    }
    // Re-sort lines top-to-bottom by their (possibly merged) top edge.
    lines.sort_by(|a, b| b.top().partial_cmp(&a.top()).unwrap_or(std::cmp::Ordering::Equal));
    lines
}

fn line_infos(page: &PdfPage) -> anyhow::Result<Vec<LineInfo>> {
    let page_height = page.height().value;
    let lines = group_lines(collect_runs(page)?);
    let mut out = Vec::new();
    for (index, line) in lines.iter().enumerate() {
        let left = line.runs.iter().map(|r| r.left).fold(f32::MAX, f32::min);
        let right = line.runs.iter().map(|r| r.right).fold(f32::MIN, f32::max);
        let first = &line.runs[0];
        let mut text = String::new();
        let mut prev_right = f32::MIN;
        for run in &line.runs {
            // Visible gap between runs becomes a single space.
            if prev_right > f32::MIN && run.left - prev_right > first.font_size.max(4.0) * 0.25 {
                text.push(' ');
            }
            text.push_str(&run.text);
            prev_right = run.right;
        }
        out.push(LineInfo {
            index,
            text,
            x: left,
            y: page_height - line.top(),
            w: right - left,
            h: line.top() - line.bottom(),
            font_size: first.font_size,
            color: first.color,
            objects: line.runs.iter().map(|r| r.object_index).collect(),
        });
    }
    Ok(out)
}

pub fn list_lines(doc: &PdfDocument, page_index: u16) -> anyhow::Result<Vec<LineInfo>> {
    let page = doc.pages().get(page_index)?;
    line_infos(&page)
}

fn get_line(lines: &[Line], index: usize) -> anyhow::Result<&Line> {
    lines
        .get(index)
        .ok_or_else(|| anyhow::anyhow!("line index {index} out of range"))
}

/// Replace a line's text. The leftmost run keeps its object (and thus font,
/// size, color, position); any other runs of the line are removed so the new
/// text doesn't overprint leftovers. The anchor font must cover the new
/// text's glyphs — characters it lacks render as blank.
pub fn edit_line(
    pdfium: &Pdfium,
    path: &Path,
    page_index: u16,
    line_index: usize,
    text: &str,
) -> anyhow::Result<()> {
    with_document(pdfium, path, |doc| {
        let mut page = doc.pages().get(page_index)?;
        let lines = group_lines(collect_runs(&page)?);
        let line = get_line(&lines, line_index)?;
        let anchor = line.runs[0].object_index;
        let mut extras: Vec<usize> = line.runs[1..].iter().map(|r| r.object_index).collect();

        let mut object = page.objects().get(anchor)?;
        match object.as_text_object_mut() {
            Some(text_obj) => text_obj.set_text(text)?,
            None => anyhow::bail!("line anchor object {anchor} is not a text object"),
        }
        page.regenerate_content()?;

        // Highest index first so earlier indices stay valid while deleting.
        extras.sort_unstable_by(|a, b| b.cmp(a));
        for extra in extras {
            let object = page.objects().get(extra)?;
            // See objects::delete_object: the removed wrapper must be leaked —
            // dropping it destroys a handle the content regeneration already
            // invalidated and crashes the process.
            let removed = page.objects_mut().remove_object(object)?;
            std::mem::forget(removed);
        }
        Ok(())
    })
}

/// Insert a new line of text below line `after`, copying its style (font
/// size, fill color, left edge). Leading comes from the gap to the next
/// line when there is one, else 1.35× the font size. With `shift_down`,
/// every object fully below the reference line first moves down by the
/// leading so the new line lands in cleared space (no reflow — objects that
/// fall off the page bottom are simply clipped by the page).
pub fn insert_line(
    pdfium: &Pdfium,
    path: &Path,
    page_index: u16,
    req: &InsertLine,
) -> anyhow::Result<()> {
    if req.text.trim().is_empty() {
        anyhow::bail!("text must not be empty");
    }
    // Font token must be taken before the page borrows the document
    // (same constraint as annots::create_on_doc).
    with_document(pdfium, path, |doc| {
        let font = super::font::full_font_bytes()
            .and_then(|full| {
                super::font::subset_for_text(full, &req.text)
                    .map_err(|e| tracing::warn!("font subset failed: {e}"))
                    .ok()
            })
            .and_then(|subset| {
                doc.fonts_mut()
                    .load_true_type_from_bytes(&subset, true)
                    .map_err(|e| tracing::warn!("subset font load failed: {e:?}"))
                    .ok()
            })
            .unwrap_or_else(|| doc.fonts_mut().helvetica());

        let mut page = doc.pages().get(page_index)?;
        let lines = group_lines(collect_runs(&page)?);
        let line = get_line(&lines, req.after)?;
        let anchor = &line.runs[0];
        let font_size = if anchor.font_size > 0.0 { anchor.font_size } else { 12.0 };
        let leading = match lines.get(req.after + 1) {
            // Next line's baseline gap, when it looks like normal leading.
            Some(next) if line.bottom() - next.bottom() > 0.0
                && line.bottom() - next.bottom() < font_size * 3.0 =>
            {
                line.bottom() - next.bottom()
            }
            _ => font_size * 1.35,
        };
        let ref_bottom = line.bottom();

        if req.shift_down {
            // Move everything strictly below the reference line down. Bounds
            // must be collected before mutating: translate invalidates the
            // iteration order guarantees.
            let mut below: Vec<usize> = Vec::new();
            for (i, object) in page.objects().iter().enumerate() {
                let Ok(bounds) = object.bounds() else { continue };
                if bounds.top().value < ref_bottom {
                    below.push(i);
                }
            }
            for i in below {
                let mut object = page.objects().get(i)?;
                object.translate(PdfPoints::new(0.0), PdfPoints::new(-leading))?;
            }
        }

        let mut text_obj = PdfPageTextObject::new(doc, &req.text, font, PdfPoints::new(font_size))?;
        text_obj.set_fill_color(PdfColor::new(
            anchor.color[0],
            anchor.color[1],
            anchor.color[2],
            anchor.color[3],
        ))?;
        // New baseline one leading below the reference line's bottom edge
        // (the bottom already includes the descender; close enough for a
        // no-reflow insert).
        text_obj.translate(
            PdfPoints::new(anchor.left),
            PdfPoints::new(ref_bottom - leading),
        )?;
        page.objects_mut().add_text_object(text_obj)?;
        page.regenerate_content()?;
        Ok(())
    })
}

/// Shift a line (and optionally every line below it) vertically by
/// `delta` points, positive meaning down the page. Pure translation —
/// nothing reflows and collisions with untouched content are allowed.
pub fn shift_line(
    pdfium: &Pdfium,
    path: &Path,
    page_index: u16,
    line_index: usize,
    req: &ShiftLine,
) -> anyhow::Result<()> {
    if req.delta == 0.0 || !req.delta.is_finite() {
        anyhow::bail!("delta must be a non-zero finite number of points");
    }
    with_document(pdfium, path, |doc| {
        let mut page = doc.pages().get(page_index)?;
        let lines = group_lines(collect_runs(&page)?);
        let line = get_line(&lines, line_index)?;
        let mut targets: Vec<usize> = line.runs.iter().map(|r| r.object_index).collect();
        // Reading order is top-to-bottom; every later line is "below". Do not
        // filter by geometry — adjacent lines often share a baseline edge
        // (other.top() >= ref_bottom) and would be skipped by a strict < check.
        if req.and_below {
            for other in &lines[line_index + 1..] {
                targets.extend(other.runs.iter().map(|r| r.object_index));
            }
        }
        for i in targets {
            let mut object = page.objects().get(i)?;
            object.translate(PdfPoints::new(0.0), PdfPoints::new(-req.delta))?;
        }
        page.regenerate_content()?;
        Ok(())
    })
}
