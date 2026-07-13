//! Page-level operations: rotate, delete, insert, reorder, merge, extract,
//! crop. Reorder/merge/extract build a fresh document and copy pages across,
//! because PDFium has no in-place page-move API. Crop is a lopdf pass:
//! PDFium exposes no box-editing API, and /CropBox is a plain dict entry.

use std::path::Path;

use lopdf::{Document, Object, ObjectId};
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

/// Insert pages copied from another document at `at` (0-based, may equal
/// the destination page count to append). `src_pages` are 0-based indices
/// into the source and keep the given order. Both files are loaded from
/// bytes so no open handle blocks the atomic rename.
pub fn insert_from(
    pdfium: &Pdfium,
    dst_path: &Path,
    src_path: &Path,
    src_pages: &[u16],
    at: u16,
) -> anyhow::Result<()> {
    if src_pages.is_empty() {
        anyhow::bail!("insert needs at least one source page");
    }
    let dst_bytes = std::fs::read(dst_path)?;
    let src_bytes = std::fs::read(src_path)?;
    let mut dst = pdfium.load_pdf_from_byte_vec(dst_bytes, None)?;
    let src = pdfium.load_pdf_from_byte_vec(src_bytes, None)?;

    let dst_count = dst.pages().len();
    if at > dst_count {
        anyhow::bail!("insert index {at} out of range (0..={dst_count})");
    }
    let src_count = src.pages().len();
    for &i in src_pages {
        if i >= src_count {
            anyhow::bail!("source page index {i} out of range (0..{src_count})");
        }
    }

    for (offset, &src_index) in src_pages.iter().enumerate() {
        dst.pages_mut()
            .copy_page_from_document(&src, src_index, at + offset as u16)?;
    }

    let saved = dst.save_to_bytes()?;
    drop(dst);
    drop(src);
    let tmp = dst_path.with_extension("pdf.tmp");
    std::fs::write(&tmp, &saved)?;
    std::fs::rename(&tmp, dst_path)?;
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

/// Crop rectangle in *view space*: points relative to the rendered page as
/// the user sees it (after /Rotate and any existing crop), origin top-left,
/// y growing downward. The frontend divides pixel coordinates by the render
/// scale and sends points; rotation handling stays server-side.
#[derive(Debug, Clone, Copy, serde::Deserialize)]
pub struct CropRect {
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
}

const MIN_CROP_PTS: f32 = 4.0;

/// Set /CropBox on the given pages (0-based). `rect == None` resets the
/// crop by writing /CropBox = /MediaBox (deleting the key could re-expose
/// an inherited parent /CropBox, so an explicit full-page box is safer).
///
/// The view rect is interpreted relative to the page's current *effective*
/// box (CropBox ∩ MediaBox), so cropping an already-cropped page narrows
/// further, matching what the user sees. Resulting boxes are clamped to the
/// MediaBox and must keep at least 4×4 pt.
pub fn crop(path: &Path, pages: &[u16], rect: Option<CropRect>) -> anyhow::Result<()> {
    if pages.is_empty() {
        anyhow::bail!("crop needs at least one page");
    }
    let mut doc = Document::load(path)?;
    // get_pages(): 1-based page number -> object id.
    let page_map = doc.get_pages();
    let count = page_map.len() as u16;

    let mut targets: Vec<ObjectId> = Vec::with_capacity(pages.len());
    for &i in pages {
        let id = page_map
            .get(&(u32::from(i) + 1))
            .copied()
            .ok_or_else(|| anyhow::anyhow!("page index {i} out of range (0..{count})"))?;
        targets.push(id);
    }

    for page_id in targets {
        let media = inherited_rect(&doc, page_id, b"MediaBox")
            .unwrap_or([0.0, 0.0, DEFAULT_PAGE_W, DEFAULT_PAGE_H]);

        let new_box = match rect {
            None => media,
            Some(r) => {
                let base = match inherited_rect(&doc, page_id, b"CropBox") {
                    Some(c) => intersect(c, media).unwrap_or(media),
                    None => media,
                };
                let rotation = inherited_rotation(&doc, page_id);
                view_rect_to_user_box(r, base, media, rotation)?
            }
        };

        let arr = new_box
            .iter()
            .map(|v| Object::Real(*v))
            .collect::<Vec<_>>();
        doc.get_dictionary_mut(page_id)?
            .set("CropBox", Object::Array(arr));
    }

    let mut bytes = Vec::new();
    doc.save_to(&mut bytes)?;
    let tmp = path.with_extension("pdf.tmp");
    std::fs::write(&tmp, &bytes)?;
    std::fs::rename(&tmp, path)?;
    Ok(())
}

/// Map a view-space rect onto PDF user space and clamp it to the MediaBox.
///
/// View space is the page as displayed: the base box rendered under
/// `rotation` degrees clockwise, origin top-left, y down. PDF user space
/// has origin bottom-left, y up. The mapping first flips y (top-left ->
/// bottom-left origin), then inverts the display rotation.
fn view_rect_to_user_box(
    r: CropRect,
    base: [f32; 4],
    media: [f32; 4],
    rotation: u16,
) -> anyhow::Result<[f32; 4]> {
    if r.w <= 0.0 || r.h <= 0.0 {
        anyhow::bail!("crop rect must have positive size");
    }
    let (bw, bh) = (base[2] - base[0], base[3] - base[1]);
    // Displayed dimensions swap for 90/270.
    let disp_h = if rotation == 90 || rotation == 270 { bw } else { bh };

    // Flip y: rect corners in display space with a bottom-left origin.
    let corners = [
        (r.x, disp_h - r.y - r.h),
        (r.x + r.w, disp_h - r.y),
    ];

    // Invert the clockwise display rotation to get offsets (a, b) within
    // the base box in user space. Derived corner-by-corner; e.g. for
    // /Rotate 90 the forward map is (a, b) -> (b, bw - a).
    let mut xs = [0f32; 2];
    let mut ys = [0f32; 2];
    for (i, (dx, dy)) in corners.into_iter().enumerate() {
        let (a, b) = match rotation {
            0 => (dx, dy),
            90 => (bw - dy, dx),
            180 => (bw - dx, bh - dy),
            270 => (dy, bh - dx),
            _ => unreachable!("rotation normalized to 0/90/180/270"),
        };
        xs[i] = base[0] + a;
        ys[i] = base[1] + b;
    }

    let unclamped = [
        xs[0].min(xs[1]),
        ys[0].min(ys[1]),
        xs[0].max(xs[1]),
        ys[0].max(ys[1]),
    ];
    let clamped = intersect(unclamped, media)
        .ok_or_else(|| anyhow::anyhow!("crop rect lies outside the page"))?;
    if clamped[2] - clamped[0] < MIN_CROP_PTS || clamped[3] - clamped[1] < MIN_CROP_PTS {
        anyhow::bail!("crop rect too small: minimum is {MIN_CROP_PTS}x{MIN_CROP_PTS} pt");
    }
    Ok(clamped)
}

fn intersect(a: [f32; 4], b: [f32; 4]) -> Option<[f32; 4]> {
    let out = [
        a[0].max(b[0]),
        a[1].max(b[1]),
        a[2].min(b[2]),
        a[3].min(b[3]),
    ];
    (out[0] < out[2] && out[1] < out[3]).then_some(out)
}

/// Look up an inheritable page attribute (/MediaBox, /CropBox, /Rotate),
/// walking the /Parent chain like a PDF reader does.
fn inherited_attr(doc: &Document, page_id: ObjectId, key: &[u8]) -> Option<Object> {
    let mut current = page_id;
    // Depth guard against malformed /Parent cycles.
    for _ in 0..64 {
        let dict = doc.get_dictionary(current).ok()?;
        if let Ok(obj) = dict.get(key) {
            return match obj {
                Object::Reference(r) => doc.get_object(*r).ok().cloned(),
                other => Some(other.clone()),
            };
        }
        match dict.get(b"Parent") {
            Ok(Object::Reference(r)) => current = *r,
            _ => return None,
        }
    }
    None
}

/// Inherited rect attribute, normalized so x0<=x1 and y0<=y1.
fn inherited_rect(doc: &Document, page_id: ObjectId, key: &[u8]) -> Option<[f32; 4]> {
    let obj = inherited_attr(doc, page_id, key)?;
    let arr = obj.as_array().ok()?;
    if arr.len() != 4 {
        return None;
    }
    let mut v = [0f32; 4];
    for (i, o) in arr.iter().enumerate() {
        let resolved = match o {
            Object::Reference(r) => doc.get_object(*r).ok()?,
            other => other,
        };
        v[i] = match resolved {
            Object::Integer(n) => *n as f32,
            Object::Real(n) => *n,
            _ => return None,
        };
    }
    Some([
        v[0].min(v[2]),
        v[1].min(v[3]),
        v[0].max(v[2]),
        v[1].max(v[3]),
    ])
}

/// Inherited /Rotate, normalized to 0/90/180/270 (spec allows negatives
/// and multiples of 360; non-multiples of 90 are treated as 0).
/// Accepts Integer or Real — some writers emit `90.0`.
fn inherited_rotation(doc: &Document, page_id: ObjectId) -> u16 {
    let raw = match inherited_attr(doc, page_id, b"Rotate") {
        Some(Object::Integer(n)) => n,
        Some(Object::Real(n)) => n.round() as i64,
        _ => return 0,
    };
    let norm = raw.rem_euclid(360);
    match norm {
        90 | 180 | 270 => norm as u16,
        _ => 0,
    }
}

#[derive(Debug, Clone, Copy, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ResizeMode {
    /// Scale page content to fit the new size (uniform, centered).
    Scale,
    /// Change the page box only; content keeps its size, centered on the
    /// new canvas (grows margins or trims edges).
    Canvas,
}

// PDF spec limits page dimensions to 3..14400 pt; we require half an inch
// so pages stay usable.
const MIN_PAGE_PTS: f32 = 36.0;
const MAX_PAGE_PTS: f32 = 14_400.0;

/// Resize the given pages (0-based) to `width` x `height` points, where the
/// target size is interpreted in *display* orientation: for pages rotated
/// 90/270 the dimensions are swapped internally so "A4 portrait" comes out
/// portrait on screen.
///
/// `Scale` wraps the content in a `q <matrix> cm ... Q` pair via two fresh
/// content streams (existing streams are untouched, so streams shared
/// between pages stay safe) and applies the same matrix to annotation
/// coordinates. `Canvas` only rewrites the boxes around the old center.
pub fn resize(
    path: &Path,
    pages: &[u16],
    width: f32,
    height: f32,
    mode: ResizeMode,
) -> anyhow::Result<()> {
    if pages.is_empty() {
        anyhow::bail!("resize needs at least one page");
    }
    for v in [width, height] {
        if !(MIN_PAGE_PTS..=MAX_PAGE_PTS).contains(&v) {
            anyhow::bail!("page size {v} pt out of range ({MIN_PAGE_PTS}..={MAX_PAGE_PTS})");
        }
    }
    let mut doc = Document::load(path)?;
    let page_map = doc.get_pages();
    let count = page_map.len() as u16;
    let mut targets: Vec<ObjectId> = Vec::with_capacity(pages.len());
    for &i in pages {
        let id = page_map
            .get(&(u32::from(i) + 1))
            .copied()
            .ok_or_else(|| anyhow::anyhow!("page index {i} out of range (0..{count})"))?;
        targets.push(id);
    }

    for page_id in targets {
        let media = inherited_rect(&doc, page_id, b"MediaBox")
            .unwrap_or([0.0, 0.0, DEFAULT_PAGE_W, DEFAULT_PAGE_H]);
        let rotation = inherited_rotation(&doc, page_id);
        // Target in user space: displayed w/h swap for rotated pages.
        let (tw, th) = if rotation == 90 || rotation == 270 {
            (height, width)
        } else {
            (width, height)
        };

        match mode {
            ResizeMode::Canvas => {
                let cx = (media[0] + media[2]) / 2.0;
                let cy = (media[1] + media[3]) / 2.0;
                let new_box = [cx - tw / 2.0, cy - th / 2.0, cx + tw / 2.0, cy + th / 2.0];
                set_rect(&mut doc, page_id, b"MediaBox", new_box)?;
                // A previous crop is meaningless on the new canvas; show it all.
                set_rect(&mut doc, page_id, b"CropBox", new_box)?;
            }
            ResizeMode::Scale => {
                let (ow, oh) = (media[2] - media[0], media[3] - media[1]);
                let s = (tw / ow).min(th / oh);
                // Map the old MediaBox onto [0,0,tw,th], centered.
                let e = (tw - s * ow) / 2.0 - s * media[0];
                let f = (th - s * oh) / 2.0 - s * media[1];
                let new_box = [0.0, 0.0, tw, th];

                let crop = inherited_rect(&doc, page_id, b"CropBox")
                    .and_then(|c| intersect(c, media))
                    .map(|c| {
                        [
                            s * c[0] + e,
                            s * c[1] + f,
                            s * c[2] + e,
                            s * c[3] + f,
                        ]
                    })
                    .unwrap_or(new_box);

                wrap_page_content(&mut doc, page_id, s, e, f)?;
                transform_page_annotations(&mut doc, page_id, s, e, f);
                set_rect(&mut doc, page_id, b"MediaBox", new_box)?;
                set_rect(&mut doc, page_id, b"CropBox", crop)?;
            }
        }
    }

    let mut bytes = Vec::new();
    doc.save_to(&mut bytes)?;
    let tmp = path.with_extension("pdf.tmp");
    std::fs::write(&tmp, &bytes)?;
    std::fs::rename(&tmp, path)?;
    Ok(())
}

fn set_rect(
    doc: &mut Document,
    page_id: ObjectId,
    key: &[u8],
    rect: [f32; 4],
) -> anyhow::Result<()> {
    let arr = rect.iter().map(|v| Object::Real(*v)).collect::<Vec<_>>();
    doc.get_dictionary_mut(page_id)?
        .set(key.to_vec(), Object::Array(arr));
    Ok(())
}

/// Wrap the page's content streams in `q <s 0 0 s e f> cm ... Q` by
/// inserting a prefix and a suffix stream around the existing /Contents.
/// Existing stream objects are left untouched.
fn wrap_page_content(
    doc: &mut Document,
    page_id: ObjectId,
    s: f32,
    e: f32,
    f: f32,
) -> anyhow::Result<()> {
    let contents = doc
        .get_dictionary(page_id)?
        .get(b"Contents")
        .ok()
        .cloned();
    // Blank page (no /Contents): box change alone is enough.
    let Some(contents) = contents else {
        return Ok(());
    };
    let existing = contents_to_list(doc, contents)?;
    if existing.is_empty() {
        return Ok(());
    }

    let prefix = format!("q\n{s} 0 0 {s} {e} {f} cm\n");
    let prefix_id = doc.add_object(Object::Stream(lopdf::Stream::new(
        lopdf::Dictionary::new(),
        prefix.into_bytes(),
    )));
    let suffix_id = doc.add_object(Object::Stream(lopdf::Stream::new(
        lopdf::Dictionary::new(),
        b"\nQ\n".to_vec(),
    )));

    let mut new_list = Vec::with_capacity(existing.len() + 2);
    new_list.push(Object::Reference(prefix_id));
    new_list.extend(existing);
    new_list.push(Object::Reference(suffix_id));
    doc.get_dictionary_mut(page_id)?
        .set("Contents", Object::Array(new_list));
    Ok(())
}

/// Resolve page `/Contents` to a list of stream references (or keep an
/// inline array). Broken / unexpected types error out so Scale resize
/// cannot rewrite boxes while leaving content unscaled.
fn contents_to_list(doc: &mut Document, obj: Object) -> anyhow::Result<Vec<Object>> {
    match obj {
        Object::Array(arr) => Ok(arr),
        Object::Reference(r) => resolve_contents_ref(doc, r),
        // Rare: inline stream on the page dict — promote so wrap can ref it.
        Object::Stream(stream) => {
            let id = doc.add_object(Object::Stream(stream));
            Ok(vec![Object::Reference(id)])
        }
        _ => anyhow::bail!("page /Contents is not a stream or array"),
    }
}

fn resolve_contents_ref(doc: &Document, mut id: ObjectId) -> anyhow::Result<Vec<Object>> {
    for _ in 0..8 {
        match doc.get_object(id) {
            Ok(Object::Stream(_)) => return Ok(vec![Object::Reference(id)]),
            Ok(Object::Array(arr)) => return Ok(arr.clone()),
            Ok(Object::Reference(next)) => id = *next,
            Ok(_) => anyhow::bail!("page /Contents does not resolve to a stream or array"),
            Err(_) => anyhow::bail!("broken page /Contents reference"),
        }
    }
    anyhow::bail!("page /Contents indirection too deep")
}

/// Apply `p' = s*p + (e, f)` to the coordinate entries of every annotation
/// on the page, so annotations (incl. form widgets) follow the scaled
/// content. Appearance streams need no change: viewers fit /AP BBox to
/// /Rect. Handles both inline annot dicts and references (PDFium's save
/// writes inline dicts).
fn transform_page_annotations(doc: &mut Document, page_id: ObjectId, s: f32, e: f32, f: f32) {
    let annots = match doc.get_dictionary(page_id) {
        Ok(page) => page.get(b"Annots").ok().cloned(),
        Err(_) => return,
    };
    let (mut inline_holder, refs): (Option<(Object, bool)>, Vec<ObjectId>) = match annots {
        Some(Object::Reference(r)) => match doc.get_object(r) {
            Ok(Object::Array(arr)) => (
                Some((Object::Array(arr.clone()), true)),
                collect_refs(arr),
            ),
            _ => (None, Vec::new()),
        },
        Some(Object::Array(ref arr)) => (
            Some((Object::Array(arr.clone()), false)),
            collect_refs(arr),
        ),
        _ => (None, Vec::new()),
    };

    // Referenced annotation dicts.
    for rid in refs {
        if let Ok(dict) = doc.get_dictionary_mut(rid) {
            transform_annot_dict(dict, s, e, f);
        }
    }
    // Inline annotation dicts inside the array itself.
    if let Some((Object::Array(ref mut arr), via_ref)) = inline_holder {
        let mut touched = false;
        for entry in arr.iter_mut() {
            if let Object::Dictionary(d) = entry {
                transform_annot_dict(d, s, e, f);
                touched = true;
            }
        }
        if touched {
            let new_arr = Object::Array(arr.clone());
            if via_ref {
                if let Some(Object::Reference(r)) = doc
                    .get_dictionary(page_id)
                    .ok()
                    .and_then(|p| p.get(b"Annots").ok().cloned())
                {
                    if let Ok(slot) = doc.get_object_mut(r) {
                        *slot = new_arr;
                    }
                }
            } else if let Ok(page) = doc.get_dictionary_mut(page_id) {
                page.set("Annots", new_arr);
            }
        }
    }
}

fn collect_refs(arr: &[Object]) -> Vec<ObjectId> {
    arr.iter()
        .filter_map(|o| match o {
            Object::Reference(r) => Some(*r),
            _ => None,
        })
        .collect()
}

/// Coordinate-bearing annotation entries: flat x,y pair arrays plus
/// /InkList's nested arrays. /Rect order stays valid because s > 0.
fn transform_annot_dict(dict: &mut lopdf::Dictionary, s: f32, e: f32, f: f32) {
    for key in [
        b"Rect".as_slice(),
        b"QuadPoints".as_slice(),
        b"Vertices".as_slice(),
        b"L".as_slice(),
        b"CL".as_slice(),
    ] {
        if let Ok(Object::Array(arr)) = dict.get_mut(key) {
            transform_pairs(arr, s, e, f);
        }
    }
    if let Ok(Object::Array(lists)) = dict.get_mut(b"InkList") {
        for entry in lists.iter_mut() {
            if let Object::Array(arr) = entry {
                transform_pairs(arr, s, e, f);
            }
        }
    }
}

fn transform_pairs(arr: &mut [Object], s: f32, e: f32, f: f32) {
    for (i, o) in arr.iter_mut().enumerate() {
        let v = match &*o {
            Object::Integer(n) => *n as f32,
            Object::Real(n) => *n,
            _ => continue,
        };
        let t = if i % 2 == 0 { s * v + e } else { s * v + f };
        *o = Object::Real(t);
    }
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

#[cfg(test)]
mod tests {
    use super::*;
    use lopdf::Dictionary;

    #[test]
    fn rotation_accepts_real() {
        let mut doc = Document::with_version("1.5");
        let mut page = Dictionary::new();
        page.set("Type", "Page");
        page.set("Rotate", Object::Real(90.0));
        let id = doc.add_object(Object::Dictionary(page));
        assert_eq!(inherited_rotation(&doc, id), 90);
    }

    #[test]
    fn wrap_fails_on_broken_contents_ref() {
        let mut doc = Document::with_version("1.5");
        let mut page = Dictionary::new();
        page.set("Type", "Page");
        page.set("Contents", Object::Reference((999, 0)));
        let id = doc.add_object(Object::Dictionary(page));
        let err = wrap_page_content(&mut doc, id, 0.5, 0.0, 0.0).unwrap_err();
        assert!(
            err.to_string().contains("Contents"),
            "unexpected error: {err}"
        );
    }

    #[test]
    fn wrap_fails_on_wrong_contents_type() {
        let mut doc = Document::with_version("1.5");
        let bad = doc.add_object(Object::Integer(1));
        let mut page = Dictionary::new();
        page.set("Type", "Page");
        page.set("Contents", Object::Reference(bad));
        let id = doc.add_object(Object::Dictionary(page));
        let err = wrap_page_content(&mut doc, id, 0.5, 0.0, 0.0).unwrap_err();
        assert!(
            err.to_string().contains("Contents"),
            "unexpected error: {err}"
        );
    }
}
