//! Redaction: rasterize a page's current appearance (content, form fields,
//! annotations — everything `PdfRenderConfig` already bakes into the
//! bitmap) to a single bitmap, burn solid-black boxes into it, then replace
//! the page's `/Contents` with a single full-page image draw, and drop its
//! `/Annots` and `/Thumb` (a cached thumbnail would otherwise still show
//! the pre-redaction rendering). The redacted page keeps no extractable
//! text, no vector paths, no live form/annotation objects — only pixels.
//!
//! PDFium's page-object API has no way to delete an individual object (see
//! `pageops.rs`'s own note on `/CropBox`), so — like crop/resize — this is a
//! lopdf structural rewrite, not a PDFium one. `redact_document` is the only
//! function here that touches PDFium (to render the page); the box-burning
//! and page-replacement logic are split out as pure/lopdf-only helpers so
//! they can be unit-tested without a bound PDFium library (this crate has
//! no other precedent for a PDFium-bound `#[test]` running un-gated — see
//! `ocr.rs`'s fixture tests, which had to be `#[ignore]`'d for exactly that
//! reason).
//!
//! A *tagged* (accessible) source PDF can carry a `/StructTreeRoot` whose
//! structure elements duplicate page text independently of the content
//! stream (via `/ActualText`/`/Alt`, or bare marked-content references).
//! Rasterizing `/Contents` alone would leave that text recoverable by a
//! screen reader or a "prefer structure text" extractor, so
//! `strip_struct_tree_for_pages` walks the tree and, for any element whose
//! effective page (its own `/Pg`, or the nearest ancestor's) is one of the
//! redacted pages: strips `/ActualText`/`/Alt`, and drops any
//! marked-content/object reference (`/Type /MCR` or `/Type /OBJR`) pointing
//! at that page. An element that ends up with no children left is dropped
//! from its parent too, so empty stubs don't pile up on redacted pages.
//! `/ParentTree` (the reverse MCID lookup index) is left alone — it only
//! re-points at the same struct elements already handled here, so it can't
//! carry text on its own; a dangling entry is a tolerated null, not a leak.

use std::collections::{BTreeMap, HashSet};
use std::path::Path;

use image::{Rgb, RgbImage};
use lopdf::content::{Content, Operation};
use lopdf::{dictionary, Document, Object, ObjectId, Stream};
use pdfium_render::prelude::*;

use super::{ops, protect};

/// A redaction rectangle in PDF points, top-left origin — same convention
/// as `annots::InRect` / `imageops::ImageObjectInfo`.
#[derive(Debug, Clone)]
pub struct RedactBox {
    pub page: u16,
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
}

#[derive(Debug, Clone)]
pub struct RedactOptions {
    /// Rasterization DPI. 300 keeps burned-in text/lines crisp at print size.
    pub dpi: f32,
    /// JPEG quality (10..=100) for the flattened page image.
    pub jpeg_quality: u8,
}

impl Default for RedactOptions {
    fn default() -> Self {
        Self { dpi: 300.0, jpeg_quality: 90 }
    }
}

#[derive(Debug, Default, serde::Serialize)]
pub struct RedactStats {
    pub pages_rasterized: usize,
    pub boxes_burned: usize,
    pub objects_pruned: usize,
    /// Struct-tree elements/marked-content refs dropped because they
    /// belonged to a redacted page (0 for untagged documents).
    pub struct_elements_removed: usize,
}

/// Paint every box in `boxes_pt` solid black onto `image`. Boxes are in PDF
/// points, top-left origin; `scale` (pixels per point) maps them onto the
/// image, which PDFium also renders top-left origin at the same scale — so
/// this is a direct multiply, no page-height flip needed. Bounds round
/// outward (floor left/top, ceil right/bottom) so the painted pixels always
/// fully cover the requested area — a redaction box must never under-cover
/// due to rounding, even by half a pixel. Out-of-bounds or degenerate boxes
/// are clamped/skipped. Returns the number of boxes actually painted (i.e.
/// with positive on-image area).
fn burn_boxes(image: &mut RgbImage, boxes_pt: &[&RedactBox], scale: f32) -> usize {
    let (px_w, px_h) = image.dimensions();
    let mut painted = 0usize;
    for b in boxes_pt {
        let left = (b.x * scale).floor().clamp(0.0, px_w as f32) as u32;
        let top = (b.y * scale).floor().clamp(0.0, px_h as f32) as u32;
        let right = ((b.x + b.w) * scale).ceil().clamp(0.0, px_w as f32) as u32;
        let bottom = ((b.y + b.h) * scale).ceil().clamp(0.0, px_h as f32) as u32;
        if right <= left || bottom <= top {
            continue;
        }
        for py in top..bottom {
            for px in left..right {
                image.put_pixel(px, py, Rgb([0, 0, 0]));
            }
        }
        painted += 1;
    }
    painted
}

/// Replace `page_id`'s entire visible content with a single full-page draw
/// of `jpeg` (already the flattened+redacted bitmap), and drop its
/// `/Annots`. Does not prune — callers batching multiple pages should call
/// `doc.prune_objects()` once after all pages are flattened.
fn flatten_page(
    doc: &mut Document,
    page_id: ObjectId,
    jpeg: Vec<u8>,
    px_w: u32,
    px_h: u32,
    page_w: f32,
    page_h: f32,
) -> anyhow::Result<()> {
    let image_dict = dictionary! {
        "Type" => "XObject",
        "Subtype" => "Image",
        "Width" => px_w as i64,
        "Height" => px_h as i64,
        "ColorSpace" => "DeviceRGB",
        "BitsPerComponent" => 8,
        "Filter" => "DCTDecode",
    };
    // Already-compressed JPEG bytes — wrapping in Flate would only cost time.
    let image_id = doc.add_object(Stream::new(image_dict, jpeg).with_compression(false));

    let content = Content {
        operations: vec![
            Operation::new("q", vec![]),
            Operation::new(
                "cm",
                vec![
                    Object::Real(page_w),
                    Object::Real(0.0),
                    Object::Real(0.0),
                    Object::Real(page_h),
                    Object::Real(0.0),
                    Object::Real(0.0),
                ],
            ),
            Operation::new("Do", vec![Object::Name(b"ImRedact".to_vec())]),
            Operation::new("Q", vec![]),
        ],
    };
    let content_id = doc.add_object(Stream::new(dictionary! {}, content.encode()?));

    let dict = doc.get_dictionary_mut(page_id)?;
    dict.set("Contents", content_id);
    dict.set(
        "Resources",
        dictionary! { "XObject" => dictionary! { "ImRedact" => image_id } },
    );
    dict.remove(b"Annots");
    // Some producers/viewers cache a page thumbnail here — if left in
    // place it would still show the pre-redaction rendering.
    dict.remove(b"Thumb");
    Ok(())
}

fn pg_to_id(obj: &Object) -> Option<ObjectId> {
    match obj {
        Object::Reference(id) => Some(*id),
        _ => None,
    }
}

/// Remove `/StructTreeRoot` content tied to any page in `redacted`. See the
/// module doc comment for the exact rule. Returns the number of elements
/// dropped (0, cheaply, for the common case of an untagged document).
fn strip_struct_tree_for_pages(doc: &mut Document, redacted: &HashSet<ObjectId>) -> usize {
    let root_id = match doc.catalog().ok().and_then(|c| c.get(b"StructTreeRoot").ok().cloned()) {
        Some(Object::Reference(id)) => id,
        _ => return 0,
    };
    let kids = match doc.get_dictionary(root_id).ok().and_then(|d| d.get(b"K").ok().cloned()) {
        Some(k) => k,
        None => return 0,
    };

    let mut removed = 0usize;
    let new_kids = filter_struct_k_value(doc, kids, None, redacted, &mut removed);
    if let Ok(dict) = doc.get_dictionary_mut(root_id) {
        match new_kids {
            Some(k) => dict.set("K", k),
            None => {
                dict.remove(b"K");
            }
        }
    }
    removed
}

/// Filter a `/K` value (bare MCID, a single MCR/OBJR/StructElem, or an
/// array of any of those) under `inherited_pg`. Returns `None` if nothing
/// survives.
fn filter_struct_k_value(
    doc: &mut Document,
    value: Object,
    inherited_pg: Option<ObjectId>,
    redacted: &HashSet<ObjectId>,
    removed: &mut usize,
) -> Option<Object> {
    match value {
        Object::Array(items) => {
            let kept: Vec<Object> = items
                .into_iter()
                .filter_map(|item| filter_struct_kid(doc, item, inherited_pg, redacted, removed))
                .collect();
            match kept.len() {
                0 => None,
                1 => kept.into_iter().next(),
                _ => Some(Object::Array(kept)),
            }
        }
        other => filter_struct_kid(doc, other, inherited_pg, redacted, removed),
    }
}

/// Filter one `/K` entry: a bare MCID integer, an `/MCR`/`/OBJR`
/// marked-content reference, or a nested struct element — addressed either
/// by indirect `Reference` or inline `Dictionary`. Returns `None` if it
/// should be dropped from its parent's `/K`.
fn filter_struct_kid(
    doc: &mut Document,
    kid: Object,
    inherited_pg: Option<ObjectId>,
    redacted: &HashSet<ObjectId>,
    removed: &mut usize,
) -> Option<Object> {
    match kid {
        Object::Integer(_) => {
            if inherited_pg.map_or(false, |pg| redacted.contains(&pg)) {
                *removed += 1;
                None
            } else {
                Some(kid)
            }
        }
        Object::Reference(id) => {
            let dict = doc.get_dictionary(id).ok()?.clone();
            let new_dict = filter_struct_dict(doc, dict, inherited_pg, redacted, removed)?;
            if let Ok(d) = doc.get_dictionary_mut(id) {
                *d = new_dict;
            }
            Some(Object::Reference(id))
        }
        Object::Dictionary(dict) => {
            let new_dict = filter_struct_dict(doc, dict, inherited_pg, redacted, removed)?;
            Some(Object::Dictionary(new_dict))
        }
        other => Some(other),
    }
}

/// Core per-element logic, shared by both `Reference` and inline
/// `Dictionary` kids. Returns `None` if the whole element should be
/// dropped; otherwise the (possibly stripped/filtered) dictionary to write
/// back.
fn filter_struct_dict(
    doc: &mut Document,
    mut dict: lopdf::Dictionary,
    inherited_pg: Option<ObjectId>,
    redacted: &HashSet<ObjectId>,
    removed: &mut usize,
) -> Option<lopdf::Dictionary> {
    let is_mc_ref = matches!(dict.get(b"Type").and_then(Object::as_name), Ok(b"MCR") | Ok(b"OBJR"));
    let own_pg = dict.get(b"Pg").ok().and_then(pg_to_id);
    let effective_pg = own_pg.or(inherited_pg);
    let on_redacted_page = effective_pg.map_or(false, |pg| redacted.contains(&pg));

    if is_mc_ref {
        return if on_redacted_page {
            *removed += 1;
            None
        } else {
            Some(dict)
        };
    }

    if on_redacted_page {
        // A leaked text description of this page's content, independent of
        // the content stream itself — must go even if children survive.
        dict.remove(b"ActualText");
        dict.remove(b"Alt");
    }

    let k = dict.get(b"K").ok().cloned();
    let new_k = k.and_then(|k| filter_struct_k_value(doc, k, effective_pg, redacted, removed));

    if new_k.is_none() {
        if on_redacted_page {
            // Nothing left under this node — whether it never had children
            // (a leaf like an ActualText-only heading) or lost them all to
            // filtering, it's entirely confined to the redacted page.
            *removed += 1;
            return None;
        }
        dict.remove(b"K");
    } else {
        dict.set("K", new_k.unwrap());
    }
    Some(dict)
}

/// Rasterize and redact every page referenced by `boxes`, embedding the
/// result as a new document (original file on disk untouched, matching
/// `compress::compress` / `ocr::ocr_document`'s shape). Pages not named by
/// any box are left completely untouched — text, vectors, and annotations
/// on those pages survive as-is.
pub fn redact_document(
    pdfium: &Pdfium,
    path: &Path,
    boxes: &[RedactBox],
    opts: &RedactOptions,
) -> anyhow::Result<(Vec<u8>, RedactStats)> {
    anyhow::ensure!(!boxes.is_empty(), "redact needs at least one box");
    protect::assert_editable(path)?;

    let mut by_page: BTreeMap<u16, Vec<&RedactBox>> = BTreeMap::new();
    for b in boxes {
        anyhow::ensure!(
            b.w > 0.0 && b.h > 0.0,
            "redact box on page {} has non-positive size",
            b.page
        );
        by_page.entry(b.page).or_default().push(b);
    }

    let mut stats = RedactStats::default();
    let scale = opts.dpi / 72.0;
    let quality = opts.jpeg_quality.clamp(10, 100);

    // Render + burn every affected page while PDFium's doc is open; collect
    // JPEG bytes now so PDFium's borrow of `path` can end before lopdf
    // re-opens the same file for the structural rewrite.
    let mut flattened: Vec<(u16, Vec<u8>, u32, u32, f32, f32)> = Vec::with_capacity(by_page.len());
    {
        let bytes = std::fs::read(path)?;
        let pdfium_doc = pdfium.load_pdf_from_byte_vec(bytes, None)?;
        let page_count = pdfium_doc.pages().len();

        for (page_index, page_boxes) in by_page {
            anyhow::ensure!(
                page_index < page_count,
                "page index {page_index} out of range (0..{page_count})"
            );
            let page = pdfium_doc.pages().get(page_index)?;
            let page_w = page.width().value;
            let page_h = page.height().value;

            let image = ops::render_page_image(&pdfium_doc, page_index, scale)?;
            let mut rgb = image::DynamicImage::ImageRgba8(image).to_rgb8();
            let (px_w, px_h) = rgb.dimensions();

            stats.boxes_burned += burn_boxes(&mut rgb, &page_boxes, scale);

            let mut jpeg = Vec::new();
            image::codecs::jpeg::JpegEncoder::new_with_quality(&mut jpeg, quality)
                .encode_image(&image::DynamicImage::ImageRgb8(rgb))?;

            flattened.push((page_index, jpeg, px_w, px_h, page_w, page_h));
            stats.pages_rasterized += 1;
        }
    }

    let mut doc = Document::load(path)?;
    let page_map = doc.get_pages();
    let mut redacted_page_ids: HashSet<ObjectId> = HashSet::new();
    for (page_index, jpeg, px_w, px_h, page_w, page_h) in flattened {
        let page_id = *page_map
            .get(&(u32::from(page_index) + 1))
            .ok_or_else(|| anyhow::anyhow!("page index {page_index} not found in lopdf page map"))?;
        flatten_page(&mut doc, page_id, jpeg, px_w, px_h, page_w, page_h)?;
        redacted_page_ids.insert(page_id);
    }
    stats.struct_elements_removed = strip_struct_tree_for_pages(&mut doc, &redacted_page_ids);
    stats.objects_pruned = doc.prune_objects().len();

    let mut out = Vec::new();
    doc.save_to(&mut out)?;
    Ok((out, stats))
}

#[cfg(test)]
mod tests {
    use super::*;
    use lopdf::StringFormat;

    fn contains_bytes(haystack: &[u8], needle: &[u8]) -> bool {
        haystack.windows(needle.len()).any(|w| w == needle)
    }

    fn temp_pdf(name: &str, doc: &mut Document) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join("pdf-editor-redact-test");
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join(name);
        doc.save(&path).unwrap();
        path
    }

    /// One 200x200pt page with a Helvetica "Hello" text run plus one
    /// annotation dict directly on /Annots (as PDFium's own writer would
    /// leave it — inline, not a reference).
    fn build_text_and_annot_pdf() -> (Document, ObjectId) {
        let mut doc = Document::with_version("1.5");
        let font_id = doc.add_object(dictionary! {
            "Type" => "Font",
            "Subtype" => "Type1",
            "BaseFont" => "Helvetica",
        });
        let content = Content {
            operations: vec![
                Operation::new("BT", vec![]),
                Operation::new("Tf", vec![Object::Name(b"F1".to_vec()), 24.into()]),
                Operation::new("Td", vec![10.into(), 100.into()]),
                Operation::new(
                    "Tj",
                    vec![Object::String(b"Hello".to_vec(), StringFormat::Literal)],
                ),
                Operation::new("ET", vec![]),
            ],
        };
        let content_id = doc.add_object(Stream::new(dictionary! {}, content.encode().unwrap()));
        let resources = dictionary! { "Font" => dictionary! { "F1" => font_id } };
        let pages_id = doc.new_object_id();
        let annot = dictionary! {
            "Type" => "Annot",
            "Subtype" => "Text",
            "Rect" => vec![10.into(), 10.into(), 50.into(), 30.into()],
            "Contents" => Object::String(b"secret note".to_vec(), StringFormat::Literal),
        };
        let thumb_id = doc.add_object(Stream::new(
            dictionary! {
                "Type" => "XObject",
                "Subtype" => "Image",
                "Width" => 10,
                "Height" => 10,
                "ColorSpace" => "DeviceGray",
                "BitsPerComponent" => 8,
            },
            vec![0u8; 100],
        ));
        let page_id = doc.add_object(dictionary! {
            "Type" => "Page",
            "Parent" => pages_id,
            "MediaBox" => vec![0.into(), 0.into(), 200.into(), 200.into()],
            "Resources" => resources,
            "Contents" => content_id,
            "Annots" => vec![Object::Dictionary(annot)],
            "Thumb" => thumb_id,
        });
        doc.objects.insert(
            pages_id,
            Object::Dictionary(dictionary! {
                "Type" => "Pages",
                "Kids" => vec![page_id.into()],
                "Count" => 1,
            }),
        );
        let catalog_id = doc.add_object(dictionary! {
            "Type" => "Catalog",
            "Pages" => pages_id,
        });
        doc.trailer.set("Root", catalog_id);
        (doc, page_id)
    }

    /// A tagged (accessible) two-page document: page 1's struct subtree
    /// carries an `/ActualText` (the classic redaction-bypass vector) plus
    /// a bare-MCID marked-content ref; page 2's subtree is independent, on
    /// a different page, and must survive untouched.
    fn build_tagged_pdf() -> (Document, ObjectId, ObjectId, ObjectId) {
        let mut doc = Document::with_version("1.7");
        let pages_id = doc.new_object_id();
        let page1_id = doc.add_object(dictionary! {
            "Type" => "Page", "Parent" => pages_id,
            "MediaBox" => vec![0.into(), 0.into(), 200.into(), 200.into()],
        });
        let page2_id = doc.add_object(dictionary! {
            "Type" => "Page", "Parent" => pages_id,
            "MediaBox" => vec![0.into(), 0.into(), 200.into(), 200.into()],
        });
        doc.objects.insert(
            pages_id,
            Object::Dictionary(dictionary! {
                "Type" => "Pages",
                "Kids" => vec![page1_id.into(), page2_id.into()],
                "Count" => 2,
            }),
        );

        let mcr1 = Object::Dictionary(dictionary! {
            "Type" => "MCR", "Pg" => page1_id, "MCID" => 0,
        });
        let heading_elem = doc.add_object(dictionary! {
            "Type" => "StructElem", "S" => "H1", "Pg" => page1_id,
            "ActualText" => Object::String(b"secret heading".to_vec(), StringFormat::Literal),
        });
        let sect1 = doc.add_object(dictionary! {
            "Type" => "StructElem", "S" => "Sect", "Pg" => page1_id,
            "K" => vec![mcr1, Object::Reference(heading_elem)],
        });

        let mcr2 = Object::Dictionary(dictionary! {
            "Type" => "MCR", "Pg" => page2_id, "MCID" => 0,
        });
        let sect2 = doc.add_object(dictionary! {
            "Type" => "StructElem", "S" => "Sect", "Pg" => page2_id,
            "K" => vec![mcr2],
        });

        let doc_elem = doc.add_object(dictionary! {
            "Type" => "StructElem", "S" => "Document",
            "K" => vec![Object::Reference(sect1), Object::Reference(sect2)],
        });
        let struct_root = doc.add_object(dictionary! {
            "Type" => "StructTreeRoot",
            "K" => vec![Object::Reference(doc_elem)],
        });
        let catalog_id = doc.add_object(dictionary! {
            "Type" => "Catalog",
            "Pages" => pages_id,
            "StructTreeRoot" => struct_root,
        });
        doc.trailer.set("Root", catalog_id);
        (doc, page1_id, page2_id, sect2)
    }

    #[test]
    fn strip_struct_tree_removes_only_the_redacted_pages_subtree() {
        let (mut doc, page1_id, _page2_id, sect2_id) = build_tagged_pdf();
        let redacted: HashSet<ObjectId> = [page1_id].into_iter().collect();

        let removed = strip_struct_tree_for_pages(&mut doc, &redacted);
        assert!(removed >= 2, "expected the MCR and the ActualText struct elem removed, got {removed}");

        let mut out = Vec::new();
        doc.prune_objects();
        doc.save_to(&mut out).unwrap();

        assert!(
            !contains_bytes(&out, b"secret heading"),
            "ActualText from the redacted page leaked into saved bytes"
        );

        // Page 2's own struct subtree is untouched: its element must still
        // resolve, with its MCR child still present.
        let reloaded = Document::load_mem(&out).unwrap();
        let sect2 = reloaded.get_dictionary(sect2_id).expect("sect2 should survive");
        assert!(sect2.get(b"K").is_ok(), "page 2's marked-content ref should survive");
    }

    #[test]
    fn burn_boxes_paints_only_the_requested_area() {
        let mut image = RgbImage::from_pixel(100, 100, Rgb([200, 200, 200]));
        let boxes = vec![RedactBox { page: 0, x: 10.0, y: 20.0, w: 30.0, h: 15.0 }];
        let refs: Vec<&RedactBox> = boxes.iter().collect();
        let painted = burn_boxes(&mut image, &refs, 1.0);
        assert_eq!(painted, 1);

        assert_eq!(*image.get_pixel(15, 25), Rgb([0, 0, 0]));
        assert_eq!(*image.get_pixel(0, 0), Rgb([200, 200, 200]));
        assert_eq!(*image.get_pixel(99, 99), Rgb([200, 200, 200]));
    }

    /// A redaction box must never under-cover its requested area due to
    /// rounding — bounds should round outward (floor left/top, ceil
    /// right/bottom), not to nearest.
    #[test]
    fn burn_boxes_rounds_outward_to_fully_cover_requested_area() {
        let mut image = RgbImage::from_pixel(20, 20, Rgb([9, 9, 9]));
        // At scale 1.0, a box from x=5.4..10.6 must blacken pixel columns
        // 5..=10 inclusive (floor(5.4)=5, ceil(10.6)=11 -> range 5..11).
        let b = RedactBox { page: 0, x: 5.4, y: 5.4, w: 5.2, h: 5.2 };
        let refs = vec![&b];
        burn_boxes(&mut image, &refs, 1.0);
        for px in 5..11u32 {
            assert_eq!(*image.get_pixel(px, 7), Rgb([0, 0, 0]), "column {px} should be covered");
        }
        assert_eq!(*image.get_pixel(4, 7), Rgb([9, 9, 9]), "column 4 must stay untouched");
    }

    #[test]
    fn burn_boxes_skips_degenerate_and_out_of_bounds() {
        let mut image = RgbImage::from_pixel(50, 50, Rgb([1, 2, 3]));
        let zero = RedactBox { page: 0, x: 10.0, y: 10.0, w: 0.0, h: 5.0 };
        let boxes = vec![&zero];
        assert_eq!(burn_boxes(&mut image, &boxes, 1.0), 0);
        // Fully clamped/out-of-range box still counts if it clamps to a
        // positive area; a box entirely past the image is skipped.
        let past = RedactBox { page: 0, x: 1000.0, y: 1000.0, w: 10.0, h: 10.0 };
        let boxes = vec![&past];
        assert_eq!(burn_boxes(&mut image, &boxes, 1.0), 0);
    }

    #[test]
    fn flatten_page_replaces_content_and_drops_annots() {
        let (mut doc, page_id) = build_text_and_annot_pdf();
        let path = temp_pdf("flatten.pdf", &mut doc);
        let mut doc = Document::load(&path).unwrap();

        let jpeg = vec![0xFFu8, 0xD8, 0xFF, 0xD9]; // not a real decodable JPEG; flatten_page never decodes it
        flatten_page(&mut doc, page_id, jpeg, 400, 400, 200.0, 200.0).unwrap();
        let pruned = doc.prune_objects().len();
        assert!(pruned >= 3, "expected old font/content/annot/thumb objects to be prunable, pruned {pruned}");

        let mut out = Vec::new();
        doc.save_to(&mut out).unwrap();

        // Byte-level check: the redacted text/annotation content must not
        // merely be unreachable, it must be physically gone from the saved
        // bytes — structural pruning alone doesn't guarantee that.
        assert!(!contains_bytes(&out, b"Hello"), "original page text leaked into saved bytes");
        assert!(!contains_bytes(&out, b"secret note"), "original annotation text leaked into saved bytes");

        let reloaded = Document::load_mem(&out).unwrap();
        let page = reloaded.get_dictionary(page_id).unwrap();
        assert!(page.get(b"Annots").is_err(), "Annots should be gone");
        assert!(page.get(b"Thumb").is_err(), "Thumb should be gone");

        let mut image_count = 0;
        for object in reloaded.objects.values() {
            if let Object::Stream(s) = object {
                if matches!(s.dict.get(b"Subtype").and_then(Object::as_name), Ok(b"Image")) {
                    image_count += 1;
                    assert_eq!(s.dict.get(b"Width").unwrap().as_i64().unwrap(), 400);
                    assert_eq!(s.dict.get(b"Height").unwrap().as_i64().unwrap(), 400);
                }
                // The old text-run content stream must no longer be reachable
                // from the page (it's still in `doc.objects` only if pruning
                // missed it, which the assert above already checked).
            }
        }
        assert_eq!(image_count, 1, "expected exactly one flattened page image");
    }
}
