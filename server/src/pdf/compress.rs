//! Pure-Rust PDF compression (no qpdf/Ghostscript).
//!
//! Pipeline: downsample + re-encode image XObjects (JPEG for color/gray,
//! Flate for soft masks), deduplicate identical image streams, prune
//! unreachable objects, then Flate-compress remaining uncompressed streams.
//!
//! Images are only downsampled when their effective DPI — pixel size vs the
//! largest size they are actually drawn at anywhere in the document — exceeds
//! the target. Drawn sizes come from walking every page's content stream
//! (tracking q/Q/cm and Do) and recursing into Form XObjects.
//!
//! Deliberately untouched: CCITT/JBIG2/JPX images (already efficient or
//! unsupported), CMYK and other exotic color spaces, indexed-palette images,
//! images with non-trivial /Decode or predictor DecodeParms, and inline
//! images (BI/ID/EI). Skipping is always safe: the original stream is kept.

use std::collections::{HashMap, HashSet};
use std::io::Write;
use std::path::Path;

use image::imageops::FilterType;
use lopdf::content::Content;
use lopdf::{Dictionary, Document, Object, ObjectId};

use super::protect;

#[derive(Debug, Clone, Copy)]
pub struct CompressOptions {
    /// Images drawn at more than ~1.25x this DPI are downsampled to it.
    pub target_dpi: f32,
    /// JPEG quality (10..=100) for re-encoded color/gray images.
    pub jpeg_quality: u8,
}

#[derive(Debug, Default, serde::Serialize)]
pub struct CompressStats {
    pub images_recompressed: usize,
    pub images_skipped: usize,
    pub duplicates_merged: usize,
    pub objects_pruned: usize,
}

/// Decompression bomb guard for a single image stream.
const MAX_IMAGE_BYTES: usize = 256 * 1024 * 1024;
/// Form XObject recursion limit.
const MAX_FORM_DEPTH: u8 = 8;
/// Hysteresis: don't downsample for marginal gains.
const DPI_SLACK: f32 = 1.25;

pub fn compress(path: &Path, opts: &CompressOptions) -> anyhow::Result<(Vec<u8>, CompressStats)> {
    // Empty-user-password (P11) PDFs auto-decrypt on `Document::load` and the
    // trailer `/Encrypt` check below would miss them — refuse first.
    protect::assert_editable(path)?;
    let mut doc = Document::load(path)?;
    if doc.trailer.get(b"Encrypt").is_ok() {
        anyhow::bail!("encrypted documents are not supported");
    }

    let mut stats = CompressStats::default();
    let usage = collect_image_usage(&doc);
    recompress_images(&mut doc, &usage, opts, &mut stats);
    dedup_image_streams(&mut doc, &mut stats);
    stats.objects_pruned = doc.prune_objects().len();
    // Flate any remaining uncompressed streams (content streams etc.).
    // Stream::compress is a no-op for streams that already have a /Filter,
    // so re-encoded DCT images are not double-compressed.
    doc.compress();

    let mut bytes = Vec::new();
    doc.save_to(&mut bytes)?;
    Ok((bytes, stats))
}

// ---------------------------------------------------------------------------
// Usage collection: how large is each image drawn, in points?

type Matrix = [f32; 6]; // [a b c d e f]

const IDENTITY: Matrix = [1.0, 0.0, 0.0, 1.0, 0.0, 0.0];

/// `m` applied before `ctm` (PDF `cm` semantics: CTM' = m × CTM).
fn mat_mul(m: Matrix, n: Matrix) -> Matrix {
    [
        m[0] * n[0] + m[1] * n[2],
        m[0] * n[1] + m[1] * n[3],
        m[2] * n[0] + m[3] * n[2],
        m[2] * n[1] + m[3] * n[3],
        m[4] * n[0] + m[5] * n[2] + n[4],
        m[4] * n[1] + m[5] * n[3] + n[5],
    ]
}

fn obj_to_f32(obj: &Object) -> Option<f32> {
    match obj {
        Object::Integer(i) => Some(*i as f32),
        Object::Real(r) => Some(*r),
        _ => None,
    }
}

/// Follow references to the underlying object.
fn resolve<'a>(doc: &'a Document, mut obj: &'a Object) -> &'a Object {
    let mut hops = 0;
    while let Object::Reference(id) = obj {
        match doc.get_object(*id) {
            Ok(o) if hops < 16 => {
                obj = o;
                hops += 1;
            }
            _ => break,
        }
    }
    obj
}

/// Largest drawn size (width_pt, height_pt) per image XObject id.
fn collect_image_usage(doc: &Document) -> HashMap<ObjectId, (f32, f32)> {
    let mut usage = HashMap::new();
    for page_id in doc.get_pages().values() {
        let resources = page_resources(doc, *page_id);
        let data = doc.get_page_content(*page_id);
        let mut visited = HashSet::new();
        walk_content(doc, &data, &resources, IDENTITY, &mut visited, &mut usage, 0);
    }
    // A soft mask is drawn wherever its parent image is drawn.
    let mut smask_usage = Vec::new();
    for (id, object) in &doc.objects {
        if let Object::Stream(s) = object {
            if let (Some(&size), Ok(Object::Reference(mask_id))) =
                (usage.get(id), s.dict.get(b"SMask"))
            {
                smask_usage.push((*mask_id, size));
            }
        }
    }
    for (id, (w, h)) in smask_usage {
        record_usage(&mut usage, id, w, h);
    }
    usage
}

fn record_usage(usage: &mut HashMap<ObjectId, (f32, f32)>, id: ObjectId, w: f32, h: f32) {
    let entry = usage.entry(id).or_insert((0.0, 0.0));
    entry.0 = entry.0.max(w);
    entry.1 = entry.1.max(h);
}

/// Effective /Resources for a page (direct wins; inheritance replaces whole).
fn page_resources(doc: &Document, page_id: ObjectId) -> Dictionary {
    match doc.get_page_resources(page_id) {
        Ok((Some(direct), _)) => direct.clone(),
        Ok((None, inherited)) => inherited
            .first()
            .and_then(|id| doc.get_dictionary(*id).ok())
            .cloned()
            .unwrap_or_default(),
        Err(_) => Dictionary::new(),
    }
}

fn walk_content(
    doc: &Document,
    data: &[u8],
    resources: &Dictionary,
    base_ctm: Matrix,
    visited: &mut HashSet<ObjectId>,
    usage: &mut HashMap<ObjectId, (f32, f32)>,
    depth: u8,
) {
    let Ok(content) = Content::decode(data) else {
        return;
    };
    let mut stack: Vec<Matrix> = Vec::new();
    let mut ctm = base_ctm;

    for op in &content.operations {
        match op.operator.as_str() {
            "q" => stack.push(ctm),
            "Q" => {
                if let Some(m) = stack.pop() {
                    ctm = m;
                }
            }
            "cm" => {
                if op.operands.len() == 6 {
                    let vals: Vec<f32> =
                        op.operands.iter().filter_map(obj_to_f32).collect();
                    if vals.len() == 6 {
                        let m = [vals[0], vals[1], vals[2], vals[3], vals[4], vals[5]];
                        ctm = mat_mul(m, ctm);
                    }
                }
            }
            "Do" => {
                let Some(Ok(name)) = op.operands.first().map(Object::as_name) else {
                    continue;
                };
                let Ok(xobjects) = resources
                    .get(b"XObject")
                    .map(|o| resolve(doc, o))
                    .and_then(|o| o.as_dict())
                else {
                    continue;
                };
                let Ok(Object::Reference(xid)) = xobjects.get(name) else {
                    continue;
                };
                let Ok(stream) = doc.get_object(*xid).and_then(Object::as_stream) else {
                    continue;
                };
                match stream.dict.get(b"Subtype").and_then(Object::as_name) {
                    Ok(b"Image") => {
                        // Unit image square maps through the CTM: the drawn
                        // edge vectors are (a,b) and (c,d).
                        let w = (ctm[0] * ctm[0] + ctm[1] * ctm[1]).sqrt();
                        let h = (ctm[2] * ctm[2] + ctm[3] * ctm[3]).sqrt();
                        record_usage(usage, *xid, w, h);
                    }
                    Ok(b"Form") if depth < MAX_FORM_DEPTH && !visited.contains(xid) => {
                        visited.insert(*xid);
                        let form_matrix = stream
                            .dict
                            .get(b"Matrix")
                            .ok()
                            .and_then(|o| o.as_array().ok())
                            .map(|a| {
                                let v: Vec<f32> =
                                    a.iter().filter_map(obj_to_f32).collect();
                                if v.len() == 6 {
                                    [v[0], v[1], v[2], v[3], v[4], v[5]]
                                } else {
                                    IDENTITY
                                }
                            })
                            .unwrap_or(IDENTITY);
                        let inner_res = stream
                            .dict
                            .get(b"Resources")
                            .map(|o| resolve(doc, o))
                            .and_then(|o| o.as_dict())
                            .map(|d| d.clone())
                            .unwrap_or_else(|_| resources.clone());
                        let inner = stream
                            .decompressed_content()
                            .unwrap_or_else(|_| stream.content.clone());
                        walk_content(
                            doc,
                            &inner,
                            &inner_res,
                            mat_mul(form_matrix, ctm),
                            visited,
                            usage,
                            depth + 1,
                        );
                        visited.remove(xid);
                    }
                    _ => {}
                }
            }
            _ => {}
        }
    }
}

// ---------------------------------------------------------------------------
// Image recompression

enum ColorClass {
    Rgb,
    Gray,
    Unsupported,
}

fn classify_colorspace(doc: &Document, cs: &Object) -> ColorClass {
    match resolve(doc, cs) {
        Object::Name(n) => match n.as_slice() {
            b"DeviceRGB" | b"CalRGB" => ColorClass::Rgb,
            b"DeviceGray" | b"CalGray" => ColorClass::Gray,
            _ => ColorClass::Unsupported,
        },
        Object::Array(items) => {
            let family = items.first().and_then(|o| o.as_name().ok());
            match family {
                Some(b"ICCBased") => {
                    let n = items
                        .get(1)
                        .map(|o| resolve(doc, o))
                        .and_then(|o| o.as_stream().ok())
                        .and_then(|s| s.dict.get(b"N").ok())
                        .and_then(|o| o.as_i64().ok());
                    match n {
                        Some(3) => ColorClass::Rgb,
                        Some(1) => ColorClass::Gray,
                        _ => ColorClass::Unsupported,
                    }
                }
                Some(b"CalRGB") => ColorClass::Rgb,
                Some(b"CalGray") => ColorClass::Gray,
                _ => ColorClass::Unsupported,
            }
        }
        _ => ColorClass::Unsupported,
    }
}

/// Which encoding path a candidate image takes.
enum SourceKind {
    /// Raw samples after Flate/LZW/ASCII decode (or no filter at all).
    Raw,
    /// A plain DCTDecode JPEG we can hand to the `image` crate.
    Jpeg,
    Skip,
}

fn source_kind(stream: &lopdf::Stream) -> SourceKind {
    let filters = stream.filters().unwrap_or_default();
    if filters.is_empty() {
        return SourceKind::Raw;
    }
    if filters
        .iter()
        .any(|f| matches!(*f, b"CCITTFaxDecode" | b"JBIG2Decode" | b"JPXDecode"))
    {
        return SourceKind::Skip;
    }
    match filters.last() {
        Some(&b"DCTDecode") if filters.len() == 1 => SourceKind::Jpeg,
        Some(&b"DCTDecode") => SourceKind::Skip, // e.g. Flate-wrapped JPEG: rare
        _ => SourceKind::Raw,                    // Flate/LZW/ASCII* chains
    }
}

/// Non-default /Decode arrays or PNG predictors are rare edge cases; skip.
fn has_tricky_params(doc: &Document, dict: &Dictionary) -> bool {
    if dict.get(b"Decode").is_ok() {
        return true;
    }
    if let Ok(parms) = dict.get(b"DecodeParms") {
        let parms = resolve(doc, parms);
        let predictor = |d: &Dictionary| {
            d.get(b"Predictor")
                .ok()
                .and_then(|o| o.as_i64().ok())
                .unwrap_or(1)
        };
        match parms {
            Object::Dictionary(d) => return predictor(d) > 1,
            Object::Array(items) => {
                for item in items {
                    if let Object::Dictionary(d) = resolve(doc, item) {
                        if predictor(d) > 1 {
                            return true;
                        }
                    }
                }
            }
            _ => {}
        }
    }
    false
}

struct Recompressed {
    content: Vec<u8>,
    filter: &'static str,
    width: u32,
    height: u32,
    colorspace: &'static str,
}

fn recompress_images(
    doc: &mut Document,
    usage: &HashMap<ObjectId, (f32, f32)>,
    opts: &CompressOptions,
    stats: &mut CompressStats,
) {
    // Soft masks get lossless Flate (JPEG ringing on an alpha channel shows
    // up as visible halos), everything else goes to JPEG.
    let mut smask_ids: HashSet<ObjectId> = HashSet::new();
    for object in doc.objects.values() {
        if let Object::Stream(s) = object {
            if let Ok(Object::Reference(id)) = s.dict.get(b"SMask") {
                smask_ids.insert(*id);
            }
        }
    }

    let ids: Vec<ObjectId> = doc.objects.keys().copied().collect();
    for id in ids {
        let result = plan_image(doc, id, usage, opts, &smask_ids);
        match result {
            Some(plan) => {
                let Ok(stream) = doc
                    .get_object_mut(id)
                    .and_then(|o| o.as_stream_mut())
                else {
                    continue;
                };
                if plan.content.len() >= stream.content.len() {
                    stats.images_skipped += 1;
                    continue;
                }
                stream.dict.set("Filter", Object::Name(plan.filter.into()));
                stream.dict.remove(b"DecodeParms");
                stream.dict.remove(b"Decode");
                stream.dict.set("Width", plan.width as i64);
                stream.dict.set("Height", plan.height as i64);
                stream.dict.set("BitsPerComponent", 8);
                stream
                    .dict
                    .set("ColorSpace", Object::Name(plan.colorspace.into()));
                stream.set_content(plan.content);
                stats.images_recompressed += 1;
            }
            None => {}
        }
    }
}

/// Decode, optionally downsample, and re-encode one image XObject.
/// Returns None when the image is not a candidate (wrong type, unsupported
/// encoding/color space, decode failure) — the original is always kept then.
fn plan_image(
    doc: &Document,
    id: ObjectId,
    usage: &HashMap<ObjectId, (f32, f32)>,
    opts: &CompressOptions,
    smask_ids: &HashSet<ObjectId>,
) -> Option<Recompressed> {
    let stream = doc.get_object(id).ok()?.as_stream().ok()?;
    let dict = &stream.dict;
    if !matches!(dict.get(b"Subtype").and_then(Object::as_name), Ok(b"Image")) {
        return None;
    }
    if matches!(dict.get(b"ImageMask").and_then(Object::as_bool), Ok(true)) {
        return None;
    }
    if has_tricky_params(doc, dict) {
        return None;
    }
    let width = dict.get(b"Width").ok().and_then(|o| o.as_i64().ok())? as u32;
    let height = dict.get(b"Height").ok().and_then(|o| o.as_i64().ok())? as u32;
    if width == 0 || height == 0 {
        return None;
    }

    let kind = source_kind(stream);
    let color = match dict.get(b"ColorSpace") {
        Ok(cs) => classify_colorspace(doc, cs),
        Err(_) => return None,
    };

    // Decode to 8-bit pixels.
    let img: image::DynamicImage = match kind {
        SourceKind::Skip => return None,
        SourceKind::Jpeg => {
            if matches!(color, ColorClass::Unsupported) {
                return None;
            }
            image::load_from_memory_with_format(&stream.content, image::ImageFormat::Jpeg)
                .ok()?
        }
        SourceKind::Raw => {
            let bpc = dict
                .get(b"BitsPerComponent")
                .ok()
                .and_then(|o| o.as_i64().ok())?;
            if bpc != 8 {
                return None;
            }
            let samples = stream.get_plain_content_with_limit(MAX_IMAGE_BYTES).ok()?;
            let expected = |n: usize| (width as usize).checked_mul(height as usize)?.checked_mul(n);
            match color {
                ColorClass::Rgb => {
                    let need = expected(3)?;
                    if samples.len() < need {
                        return None;
                    }
                    image::RgbImage::from_raw(width, height, samples[..need].to_vec())
                        .map(image::DynamicImage::ImageRgb8)?
                }
                ColorClass::Gray => {
                    let need = expected(1)?;
                    if samples.len() < need {
                        return None;
                    }
                    image::GrayImage::from_raw(width, height, samples[..need].to_vec())
                        .map(image::DynamicImage::ImageLuma8)?
                }
                ColorClass::Unsupported => return None,
            }
        }
    };

    // Downsample when drawn DPI clearly exceeds the target.
    let mut img = img;
    if let Some(&(w_pt, h_pt)) = usage.get(&id) {
        if w_pt > 0.5 && h_pt > 0.5 {
            let dpi_x = img.width() as f32 * 72.0 / w_pt;
            let dpi_y = img.height() as f32 * 72.0 / h_pt;
            if dpi_x > opts.target_dpi * DPI_SLACK || dpi_y > opts.target_dpi * DPI_SLACK {
                let new_w =
                    ((w_pt / 72.0 * opts.target_dpi).round() as u32).clamp(1, img.width());
                let new_h =
                    ((h_pt / 72.0 * opts.target_dpi).round() as u32).clamp(1, img.height());
                if new_w < img.width() || new_h < img.height() {
                    img = img.resize_exact(new_w, new_h, FilterType::Lanczos3);
                }
            }
        }
    }

    let (width, height) = (img.width(), img.height());
    let is_smask = smask_ids.contains(&id);
    let gray = matches!(color, ColorClass::Gray) || is_smask;

    if is_smask {
        // Lossless Flate for soft masks.
        let raw = img.into_luma8().into_raw();
        let mut enc =
            flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::best());
        enc.write_all(&raw).ok()?;
        let content = enc.finish().ok()?;
        return Some(Recompressed {
            content,
            filter: "FlateDecode",
            width,
            height,
            colorspace: "DeviceGray",
        });
    }

    let mut buf = Vec::new();
    let quality = opts.jpeg_quality.clamp(10, 100);
    let mut encoder = image::codecs::jpeg::JpegEncoder::new_with_quality(&mut buf, quality);
    if gray {
        encoder.encode_image(&image::DynamicImage::ImageLuma8(img.into_luma8())).ok()?;
    } else {
        encoder.encode_image(&image::DynamicImage::ImageRgb8(img.into_rgb8())).ok()?;
    }
    Some(Recompressed {
        content: buf,
        filter: "DCTDecode",
        width,
        height,
        colorspace: if gray { "DeviceGray" } else { "DeviceRGB" },
    })
}

// ---------------------------------------------------------------------------
// Duplicate image streams (same bytes embedded multiple times)

fn dedup_image_streams(doc: &mut Document, stats: &mut CompressStats) {
    let mut canonical: HashMap<(Vec<u8>, Vec<u8>), ObjectId> = HashMap::new();
    let mut remap: HashMap<ObjectId, ObjectId> = HashMap::new();

    let mut ids: Vec<ObjectId> = doc.objects.keys().copied().collect();
    ids.sort();
    for id in ids {
        let Ok(stream) = doc.get_object(id).and_then(Object::as_stream) else {
            continue;
        };
        if !matches!(
            stream.dict.get(b"Subtype").and_then(Object::as_name),
            Ok(b"Image")
        ) {
            continue;
        }
        // Key = serialized dict + raw content; identical pairs are safe to
        // merge regardless of what they encode.
        let mut dict_bytes = Vec::new();
        let mut entries: Vec<(&[u8], String)> = stream
            .dict
            .iter()
            .map(|(k, v)| (k.as_slice(), format!("{v:?}")))
            .collect();
        entries.sort();
        for (k, v) in entries {
            dict_bytes.extend_from_slice(k);
            dict_bytes.extend_from_slice(v.as_bytes());
        }
        match canonical.entry((dict_bytes, stream.content.clone())) {
            std::collections::hash_map::Entry::Occupied(e) => {
                remap.insert(id, *e.get());
            }
            std::collections::hash_map::Entry::Vacant(e) => {
                e.insert(id);
            }
        }
    }

    if remap.is_empty() {
        return;
    }
    stats.duplicates_merged = remap.len();
    doc.traverse_objects(|object| {
        if let Object::Reference(id) = object {
            if let Some(new_id) = remap.get(id) {
                *id = *new_id;
            }
        }
    });
    // The now-unreferenced duplicates fall to prune_objects afterwards.
}

#[cfg(test)]
mod tests {
    use super::*;
    use lopdf::content::Operation;
    use lopdf::{dictionary, Stream};

    /// Noise JPEG so it doesn't compress to nothing.
    fn make_jpeg(width: u32, height: u32) -> Vec<u8> {
        let img = image::RgbImage::from_fn(width, height, |x, y| {
            let v = ((x * 7919 + y * 104729) % 251) as u8;
            image::Rgb([v, v.wrapping_mul(3), v.wrapping_add(97)])
        });
        let mut buf = Vec::new();
        let mut enc = image::codecs::jpeg::JpegEncoder::new_with_quality(&mut buf, 95);
        enc.encode_image(&image::DynamicImage::ImageRgb8(img))
            .unwrap();
        buf
    }

    fn finish_single_page_pdf(
        doc: &mut Document,
        resources: Dictionary,
        content: Content,
        media: [i64; 2],
    ) {
        let content_id = doc.add_object(Stream::new(dictionary! {}, content.encode().unwrap()));
        let pages_id = doc.new_object_id();
        let page_id = doc.add_object(dictionary! {
            "Type" => "Page",
            "Parent" => pages_id,
            "MediaBox" => vec![0.into(), 0.into(), media[0].into(), media[1].into()],
            "Resources" => resources,
            "Contents" => content_id,
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
    }

    /// One page (200x200 pt) drawing the same 800x600 image twice at
    /// 100x75 pt (= 576 DPI effective) via two duplicate XObject streams.
    fn build_image_pdf() -> Document {
        let mut doc = Document::with_version("1.5");
        let jpeg = make_jpeg(800, 600);
        let image_dict = dictionary! {
            "Type" => "XObject",
            "Subtype" => "Image",
            "Width" => 800,
            "Height" => 600,
            "ColorSpace" => "DeviceRGB",
            "BitsPerComponent" => 8,
            "Filter" => "DCTDecode",
        };
        let img1 = doc.add_object(
            Stream::new(image_dict.clone(), jpeg.clone()).with_compression(false),
        );
        let img2 = doc.add_object(Stream::new(image_dict, jpeg).with_compression(false));
        let content = Content {
            operations: vec![
                Operation::new("q", vec![]),
                Operation::new(
                    "cm",
                    vec![100.into(), 0.into(), 0.into(), 75.into(), 10.into(), 10.into()],
                ),
                Operation::new("Do", vec![Object::Name(b"Im1".to_vec())]),
                Operation::new("Q", vec![]),
                Operation::new("q", vec![]),
                Operation::new(
                    "cm",
                    vec![100.into(), 0.into(), 0.into(), 75.into(), 10.into(), 100.into()],
                ),
                Operation::new("Do", vec![Object::Name(b"Im2".to_vec())]),
                Operation::new("Q", vec![]),
            ],
        };
        let resources = dictionary! {
            "XObject" => dictionary! { "Im1" => img1, "Im2" => img2 },
        };
        finish_single_page_pdf(&mut doc, resources, content, [200, 200]);
        doc
    }

    fn temp_pdf(name: &str, doc: &mut Document) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join("pdf-editor-compress-test");
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join(name);
        doc.save(&path).unwrap();
        path
    }

    #[test]
    fn downsample_dedup_and_reparse() {
        let mut doc = build_image_pdf();
        let path = temp_pdf("input.pdf", &mut doc);
        let before = std::fs::metadata(&path).unwrap().len();

        let opts = CompressOptions {
            target_dpi: 150.0,
            jpeg_quality: 75,
        };
        let (bytes, stats) = compress(&path, &opts).unwrap();

        assert!(stats.images_recompressed >= 1, "stats: {stats:?}");
        assert_eq!(stats.duplicates_merged, 1, "stats: {stats:?}");
        assert!(
            (bytes.len() as u64) < before,
            "expected shrink: {before} -> {}",
            bytes.len()
        );

        // Output must parse, keep its page, and hold one downsampled image.
        let out = Document::load_mem(&bytes).unwrap();
        assert_eq!(out.get_pages().len(), 1);
        let mut widths = Vec::new();
        for object in out.objects.values() {
            if let Object::Stream(s) = object {
                if matches!(s.dict.get(b"Subtype").and_then(Object::as_name), Ok(b"Image")) {
                    widths.push(s.dict.get(b"Width").unwrap().as_i64().unwrap());
                }
            }
        }
        assert_eq!(widths.len(), 1, "duplicates should merge: {widths:?}");
        // Drawn at 100 pt wide with target 150 DPI -> ~208 px (was 800).
        assert!(
            (150..=300).contains(&widths[0]),
            "expected ~208 px after downsample, got {}",
            widths[0]
        );
    }

    #[test]
    fn plain_text_pdf_survives_untouched() {
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
                Operation::new("Td", vec![72.into(), 700.into()]),
                Operation::new(
                    "Tj",
                    vec![Object::String(
                        b"Hello".to_vec(),
                        lopdf::StringFormat::Literal,
                    )],
                ),
                Operation::new("ET", vec![]),
            ],
        };
        let resources = dictionary! { "Font" => dictionary! { "F1" => font_id } };
        finish_single_page_pdf(&mut doc, resources, content, [612, 792]);
        let path = temp_pdf("text.pdf", &mut doc);

        let opts = CompressOptions {
            target_dpi: 150.0,
            jpeg_quality: 75,
        };
        let (bytes, stats) = compress(&path, &opts).unwrap();
        assert_eq!(stats.images_recompressed, 0);
        let out = Document::load_mem(&bytes).unwrap();
        assert_eq!(out.get_pages().len(), 1);
    }
}
