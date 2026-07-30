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
    /// /T — the comment's author.
    pub author: Option<String>,
    /// /NM of the annotation this one replies to, resolved from /IRT.
    /// `None` for top-level annotations.
    pub irt: Option<String>,
    /// /RT — "R" for a reply, "Group" for grouped markup. `None` when absent.
    pub reply_type: Option<String>,
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
        create_on_doc(doc, page_index, ann, stamp_image)
    })
}

/// Same as [create], but operates on an already-open document instead of
/// loading one from a path. Used by `compare.rs`, which burns diff
/// annotations into an in-memory document it is still assembling (a copy of
/// the "new" doc plus appended old-only pages) before ever writing it out.
pub(crate) fn create_on_doc(
    doc: &mut PdfDocument,
    page_index: u16,
    ann: &NewAnnotation,
    stamp_image: Option<image::DynamicImage>,
) -> anyhow::Result<usize> {
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
}

/// Geometry, type and contents come from the cached PDFium document; author
/// and the reply link come from a lopdf pass over the same file.
///
/// The split is forced: /IRT is a *reference* to another annotation, and
/// pdfium-render's generic dictionary getter (`get_string_value`) is
/// `pub(crate)`, so there is no way to read either /IRT or /T through the
/// handle we already hold. Measured cost of the extra parse on this machine:
/// 21 ms for the worst local sample (1 MB, 14,886 objects), 3–17 ms for the
/// rest — cheap enough for a per-page-change call, so no sidecar or cache.
/// A file lopdf cannot parse degrades to "no threading info" rather than
/// failing the whole list.
pub fn list(
    doc: &PdfDocument,
    path: &Path,
    page_index: u16,
) -> anyhow::Result<Vec<AnnotationInfo>> {
    let page = doc.pages().get(page_index)?;
    let page_height = page.height().value;
    let threads = thread_info(path, page_index).unwrap_or_default();
    let mut out = Vec::new();
    for (index, annot) in page.annotations().iter().enumerate() {
        let nm = annot.name();
        let extra = nm.as_deref().and_then(|n| threads.get(n));
        out.push(AnnotationInfo {
            index,
            nm: nm.clone(),
            kind: format!("{:?}", annot.annotation_type()),
            rect: annot.bounds().ok().map(|b| from_pdf_rect(&b, page_height)),
            contents: annot.contents(),
            author: extra.and_then(|e| e.author.clone()),
            irt: extra.and_then(|e| e.irt.clone()),
            reply_type: extra.and_then(|e| e.reply_type.clone()),
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

// ---------- 註解編輯與回覆串（4a）----------
//
// 全部走 lopdf 字典手術，理由跟 `ensure_annotation_names` 一樣：pdfium-render
// 0.8 沒有公開的註解字典寫入口（`set_string_value` 是 crate-private），而
// /IRT 更是一個「指向另一個註解物件」的間接參照，PDFium 的 API 完全碰不到。
//
// 這裡最容易踩的坑：**PDFium 存檔時把註解寫成 /Annots 陣列裡的 inline
// dictionary，不是間接物件**（同一個坑在 `ensure_annotation_names` 也記過）。
// inline dict 沒有物件編號，就無法被 /IRT 參照——所以任何要當回覆父節點的
// 註解，必須先被「提升」成間接物件。`normalize_page_annots` 做的就是這件事。

use std::collections::HashMap;

/// 從 lopdf 讀出、但 PDFium 的 handle 拿不到的欄位。
#[derive(Default, Clone)]
struct ThreadInfo {
    author: Option<String>,
    irt: Option<String>,
    reply_type: Option<String>,
}

/// PDF 字串可能是 PDFDocEncoding 或帶 BOM 的 UTF-16BE（PDF 32000-1 7.9.2.2）。
/// `contents()` 那條路 PDFium 已經處理過，但 /T、/NM 是我們自己讀的。
fn decode_pdf_string(raw: &[u8]) -> String {
    if raw.len() >= 2 && raw[0] == 0xFE && raw[1] == 0xFF {
        let units: Vec<u16> = raw[2..]
            .chunks_exact(2)
            .map(|c| u16::from_be_bytes([c[0], c[1]]))
            .collect();
        String::from_utf16_lossy(&units)
    } else {
        // PDFDocEncoding 的 ASCII 區與 UTF-8 相同；非 ASCII 的罕見字元退化成
        // lossy，總比整筆讀不出來好。
        String::from_utf8_lossy(raw).into_owned()
    }
}

/// `decode_pdf_string` 的反向。**寫入端一定要走這裡**：PDF 文字字串只能是
/// PDFDocEncoding 或帶 BOM 的 UTF-16BE，直接塞 UTF-8 bytes 會被閱讀器當成
/// PDFDocEncoding 解讀，中文就變成亂碼（實測過：「改過的留言」→「æﬂ¹é†”çı—çŁŽè¨•」）。
/// 純 ASCII 走字面字串比較好讀、檔案也小；只要有一個非 ASCII 字元就整串轉 UTF-16BE。
fn pdf_text_string(s: &str) -> lopdf::Object {
    if s.is_ascii() {
        return lopdf::Object::string_literal(s);
    }
    let mut bytes = vec![0xFE, 0xFF];
    for unit in s.encode_utf16() {
        bytes.extend_from_slice(&unit.to_be_bytes());
    }
    lopdf::Object::String(bytes, lopdf::StringFormat::Literal)
}

fn dict_string(dict: &lopdf::Dictionary, key: &[u8]) -> Option<String> {
    dict.get(key)
        .ok()
        .and_then(|o| o.as_str().ok())
        .map(decode_pdf_string)
}

fn page_object_id(doc: &lopdf::Document, page_index: u16) -> anyhow::Result<lopdf::ObjectId> {
    // lopdf 的頁碼是 1-based，對外 API 一律 0-based。
    doc.get_pages()
        .get(&(u32::from(page_index) + 1))
        .copied()
        .ok_or_else(|| anyhow::anyhow!("page index {page_index} out of range"))
}

/// 讀出某頁 /Annots 的內容。回傳 (物件編號, 字典)；inline dict 沒有編號故為 None。
/// /Annots 本身可能直接掛在頁面字典上，也可能是一個指向陣列的參照，兩種都要處理。
fn read_page_annots(
    doc: &lopdf::Document,
    page_index: u16,
) -> anyhow::Result<Vec<(Option<lopdf::ObjectId>, lopdf::Dictionary)>> {
    use lopdf::Object;

    let page_id = page_object_id(doc, page_index)?;
    let entries: Vec<Object> = match doc.get_dictionary(page_id)?.get(b"Annots") {
        Ok(Object::Array(arr)) => arr.clone(),
        Ok(Object::Reference(r)) => match doc.get_object(*r) {
            Ok(Object::Array(arr)) => arr.clone(),
            _ => Vec::new(),
        },
        _ => Vec::new(),
    };

    let mut out = Vec::new();
    for entry in entries {
        match entry {
            Object::Dictionary(d) => out.push((None, d)),
            Object::Reference(r) => {
                if let Ok(d) = doc.get_dictionary(r) {
                    out.push((Some(r), d.clone()));
                }
            }
            _ => {}
        }
    }
    Ok(out)
}

/// /NM → 作者與回覆連結。純讀取，不改檔案，也不做 inline→間接的提升——
/// inline dict 本來就不可能是別人的父節點（沒有編號可被參照），所以只讀不寫
/// 不會漏掉任何一條真的存在的回覆關係。
fn thread_info(path: &Path, page_index: u16) -> anyhow::Result<HashMap<String, ThreadInfo>> {
    let doc = lopdf::Document::load(path)?;
    let entries = read_page_annots(&doc, page_index)?;

    // 先建 物件編號 → /NM，才能把 /IRT 的參照解回對外用的穩定 id。
    let mut nm_by_id: HashMap<lopdf::ObjectId, String> = HashMap::new();
    for (id, dict) in &entries {
        if let (Some(id), Some(nm)) = (id, dict_string(dict, b"NM")) {
            nm_by_id.insert(*id, nm);
        }
    }

    let mut out = HashMap::new();
    for (_, dict) in &entries {
        let Some(nm) = dict_string(dict, b"NM") else {
            continue;
        };
        let irt = dict
            .get(b"IRT")
            .ok()
            .and_then(|o| o.as_reference().ok())
            .and_then(|r| nm_by_id.get(&r).cloned());
        let reply_type = dict
            .get(b"RT")
            .ok()
            .and_then(|o| o.as_name().ok())
            .map(|n| String::from_utf8_lossy(n).into_owned());
        out.insert(
            nm,
            ThreadInfo {
                author: dict_string(dict, b"T"),
                irt,
                reply_type,
            },
        );
    }
    Ok(out)
}

/// 把某頁 /Annots 內所有 inline dictionary 提升成間接物件，回傳全部註解的物件編號。
///
/// 這是加回覆的前置條件：/IRT 只能是參照，而 PDFium 存檔後父註解通常是 inline。
/// 提升本身是合法且無副作用的結構調整（其他 PDF 產生器本來就這樣寫），提升後
/// /Annots 一律是參照陣列，後續操作也不必再分兩種情況處理。
fn normalize_page_annots(
    doc: &mut lopdf::Document,
    page_index: u16,
) -> anyhow::Result<Vec<lopdf::ObjectId>> {
    use lopdf::Object;

    let page_id = page_object_id(doc, page_index)?;

    // /Annots 可能直接掛在頁面上，也可能是指向陣列物件的參照；先取出擁有權再改，
    // 才不會在 add_object（需要 &mut doc）時還借著陣列。
    let (entries, array_ref): (Vec<Object>, Option<lopdf::ObjectId>) =
        match doc.get_dictionary(page_id)?.get(b"Annots") {
            Ok(Object::Array(arr)) => (arr.clone(), None),
            Ok(Object::Reference(r)) => {
                let r = *r;
                match doc.get_object(r) {
                    Ok(Object::Array(arr)) => (arr.clone(), Some(r)),
                    _ => (Vec::new(), Some(r)),
                }
            }
            _ => (Vec::new(), None),
        };

    let mut ids = Vec::with_capacity(entries.len());
    let mut promoted = Vec::with_capacity(entries.len());
    for entry in entries {
        match entry {
            Object::Reference(r) => {
                ids.push(r);
                promoted.push(Object::Reference(r));
            }
            Object::Dictionary(d) => {
                let id = doc.add_object(Object::Dictionary(d));
                ids.push(id);
                promoted.push(Object::Reference(id));
            }
            other => promoted.push(other),
        }
    }

    match array_ref {
        Some(r) => {
            if let Ok(obj) = doc.get_object_mut(r) {
                *obj = Object::Array(promoted);
            }
        }
        None => {
            doc.get_dictionary_mut(page_id)?
                .set("Annots", Object::Array(promoted));
        }
    }
    Ok(ids)
}

fn find_by_nm(
    doc: &lopdf::Document,
    ids: &[lopdf::ObjectId],
    nm: &str,
) -> Option<lopdf::ObjectId> {
    ids.iter().copied().find(|id| {
        doc.get_dictionary(*id)
            .ok()
            .and_then(|d| dict_string(d, b"NM"))
            .as_deref()
            == Some(nm)
    })
}

/// PDF 日期字串（PDF 32000-1 7.9.4）。pdf-core 沒有日期套件，自己從 UTC 秒數
/// 換算民用曆——Howard Hinnant 的 days-from-civil 反函式，標準做法。
fn pdf_now() -> String {
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);
    let days = secs.div_euclid(86_400);
    let tod = secs.rem_euclid(86_400);

    let z = days + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = (z - era * 146_097) as i64; // [0, 146096]
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365; // [0, 399]
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100); // [0, 365]
    let mp = (5 * doy + 2) / 153; // [0, 11]，3 月起算
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if m <= 2 { y + 1 } else { y };

    format!(
        "D:{:04}{:02}{:02}{:02}{:02}{:02}Z00'00'",
        y,
        m,
        d,
        tod / 3600,
        (tod % 3600) / 60,
        tod % 60
    )
}

fn save_in_place(doc: &mut lopdf::Document, path: &Path) -> anyhow::Result<()> {
    let mut bytes = Vec::new();
    doc.save_to(&mut bytes)?;
    let tmp = path.with_extension("pdf.tmp");
    std::fs::write(&tmp, &bytes)?;
    std::fs::rename(&tmp, path)?;
    Ok(())
}

/// 編輯註解的文字內容與作者。
///
/// **Stamp 一律拒絕**：文字框在本專案是 Stamp 註解，畫面上的字是烘進外觀流的
/// 文字物件，`/Contents` 只是同一份文字的副本（見 `create` 的 FreeText 分支）。
/// 只改 `/Contents` 會變成「清單顯示新字、頁面還是舊字」——那種靜默不一致比
/// 直接拒絕更難查。要真的改文字框內容得重繪外觀流，屬於後續階段。
pub fn set_contents(
    path: &Path,
    page_index: u16,
    annot_id: &str,
    contents: Option<&str>,
    author: Option<&str>,
) -> anyhow::Result<()> {
    use lopdf::Object;

    super::protect::assert_editable(path)?;
    let mut doc = lopdf::Document::load(path)?;
    let ids = normalize_page_annots(&mut doc, page_index)?;
    let target = find_by_nm(&doc, &ids, annot_id)
        .ok_or_else(|| anyhow::anyhow!("no annotation with id {annot_id} on this page"))?;

    let subtype = doc
        .get_dictionary(target)?
        .get(b"Subtype")
        .ok()
        .and_then(|o| o.as_name().ok())
        .map(|n| n.to_vec())
        .unwrap_or_default();
    if subtype == b"Stamp" {
        // 開頭那句是 server 的 `is_client_error` marker，動它要同步改那份清單，
        // 否則這個「使用者送錯對象」的情況會被歸成 500 伺服器故障。
        anyhow::bail!(
            "cannot edit contents of a stamp or text box: the text is baked into the appearance stream, so the change would not be visible"
        );
    }

    let dict = doc.get_dictionary_mut(target)?;
    if let Some(c) = contents {
        dict.set("Contents", pdf_text_string(c));
    }
    if let Some(a) = author {
        dict.set("T", pdf_text_string(a));
    }
    dict.set("M", Object::string_literal(pdf_now()));
    save_in_place(&mut doc, path)
}

/// 對某個註解新增一則回覆，回傳新回覆的 /NM。
///
/// 產生的是 PDF 規範的回覆（12.5.6.2）：`/IRT` 指向父註解、`/RT /R`。Acrobat
/// 之類的閱讀器看得懂，會把它顯示在註解面板的同一串裡，而不是頁面上多一個圖示。
///
/// `/F` 設 NoView（bit 5 = 32）：PDFium 的 `render_annotations(true)` **不**懂 IRT
/// 語意，會把回覆畫成第二個便籤圖示——本 App 的頁面預覽與「列印註解」都會中招
/// （checklist C1/34）。NoView 讓螢幕／列印都不畫外觀，但註解物件仍在，清單
/// 讀得到。刻意**不用** Hidden：Hidden 在部分閱讀器會連註解清單一起藏掉。
pub fn add_reply(
    path: &Path,
    page_index: u16,
    parent_id: &str,
    contents: &str,
    author: Option<&str>,
) -> anyhow::Result<String> {
    use lopdf::{Dictionary, Object};

    super::protect::assert_editable(path)?;
    let mut doc = lopdf::Document::load(path)?;
    let ids = normalize_page_annots(&mut doc, page_index)?;
    let parent = find_by_nm(&doc, &ids, parent_id)
        .ok_or_else(|| anyhow::anyhow!("no annotation with id {parent_id} on this page"))?;

    // 回覆沿用父註解的 /Rect：規範沒規定回覆要放哪，跟著父節點走最不會在
    // 不認得 IRT 的閱讀器裡跑到奇怪的位置。
    let rect = doc
        .get_dictionary(parent)?
        .get(b"Rect")
        .ok()
        .cloned()
        .unwrap_or(Object::Array(vec![
            0.into(),
            0.into(),
            0.into(),
            0.into(),
        ]));

    const ANNOT_FLAG_NO_VIEW: i64 = 1 << 5; // PDF 32000 Table 165

    let nm = uuid::Uuid::new_v4().to_string();
    let mut reply = Dictionary::new();
    reply.set("Type", Object::Name(b"Annot".to_vec()));
    reply.set("Subtype", Object::Name(b"Text".to_vec()));
    reply.set("Rect", rect);
    reply.set("Contents", pdf_text_string(contents));
    reply.set("NM", Object::string_literal(nm.clone()));
    reply.set("M", Object::string_literal(pdf_now()));
    reply.set("F", ANNOT_FLAG_NO_VIEW);
    reply.set("IRT", Object::Reference(parent));
    reply.set("RT", Object::Name(b"R".to_vec()));
    if let Some(a) = author {
        reply.set("T", pdf_text_string(a));
    }
    let reply_id = doc.add_object(Object::Dictionary(reply));

    // normalize_page_annots 跑完之後 /Annots 保證是參照陣列，這裡只要 append。
    let page_id = page_object_id(&doc, page_index)?;
    let array_ref = match doc.get_dictionary(page_id)?.get(b"Annots") {
        Ok(Object::Reference(r)) => Some(*r),
        _ => None,
    };
    match array_ref {
        Some(r) => {
            if let Ok(Object::Array(arr)) = doc.get_object_mut(r) {
                arr.push(Object::Reference(reply_id));
            }
        }
        None => {
            if let Ok(Object::Array(arr)) = doc.get_dictionary_mut(page_id)?.get_mut(b"Annots") {
                arr.push(Object::Reference(reply_id));
            } else {
                doc.get_dictionary_mut(page_id)?
                    .set("Annots", Object::Array(vec![Object::Reference(reply_id)]));
            }
        }
    }

    save_in_place(&mut doc, path)?;
    Ok(nm)
}

/// 刪掉所有以 `parent_nm` 為父節點的回覆，回傳刪除筆數。
///
/// 刪父註解時要先跑這個，否則回覆的 /IRT 會指向一個已經不存在的物件，清單上
/// 變成一則沒有歸屬、也沒辦法再回覆的孤兒留言。
pub fn remove_replies_to(path: &Path, page_index: u16, parent_nm: &str) -> anyhow::Result<usize> {
    use lopdf::Object;

    let doc_probe = lopdf::Document::load(path)?;
    let entries = read_page_annots(&doc_probe, page_index)?;
    let has_replies = entries.iter().any(|(_, d)| d.has(b"IRT"));
    if !has_replies {
        // 常見情況：整份文件沒有任何回覆。不要為此重寫檔案。
        return Ok(0);
    }
    drop(doc_probe);

    super::protect::assert_editable(path)?;
    let mut doc = lopdf::Document::load(path)?;
    let ids = normalize_page_annots(&mut doc, page_index)?;
    let Some(parent) = find_by_nm(&doc, &ids, parent_nm) else {
        return Ok(0);
    };

    // 回覆可以再被回覆，所以要遞移收集到不動點為止：只砍直接子節點的話，
    // 「回覆的回覆」會活下來變成孤兒——它的 /IRT 指向已刪除的物件，解不回任何
    // /NM，在清單上看起來就像一則突然冒出來的頂層留言。（實測踩過。）
    let irt_of = |doc: &lopdf::Document, id: lopdf::ObjectId| {
        doc.get_dictionary(id)
            .ok()
            .and_then(|d| d.get(b"IRT").ok())
            .and_then(|o| o.as_reference().ok())
    };
    let mut doomed: Vec<lopdf::ObjectId> = Vec::new();
    let mut frontier = vec![parent];
    while let Some(current) = frontier.pop() {
        for id in ids.iter().copied() {
            if doomed.contains(&id) || id == parent {
                continue;
            }
            if irt_of(&doc, id) == Some(current) {
                doomed.push(id);
                frontier.push(id);
            }
        }
    }
    if doomed.is_empty() {
        return Ok(0);
    }

    let page_id = page_object_id(&doc, page_index)?;
    let array_ref = match doc.get_dictionary(page_id)?.get(b"Annots") {
        Ok(Object::Reference(r)) => Some(*r),
        _ => None,
    };
    let keep = |arr: &mut Vec<Object>| {
        arr.retain(|e| !matches!(e, Object::Reference(r) if doomed.contains(r)));
    };
    match array_ref {
        Some(r) => {
            if let Ok(Object::Array(arr)) = doc.get_object_mut(r) {
                keep(arr);
            }
        }
        None => {
            if let Ok(Object::Array(arr)) = doc.get_dictionary_mut(page_id)?.get_mut(b"Annots") {
                keep(arr);
            }
        }
    }
    for id in &doomed {
        doc.objects.remove(id);
    }
    save_in_place(&mut doc, path)?;
    Ok(doomed.len())
}

/// 換註解顏色（4b）。
///
/// 幾何資料存在哪決定策略，三類完全不同：
/// - **標記類**（Highlight/Underline/StrikeOut/Squiggly）：幾何在 /QuadPoints，
///   外觀流是 PDFium 依 /C + /QuadPoints 生成的衍生物。所以改 /C 之後把 /AP
///   丟掉，下次載入 PDFium 會用新顏色重建。
/// - **手繪 Ink**：筆畫是 create 時畫進外觀流的 path 物件，**沒有 /InkList**
///   （見 create 的 Ink 分支）。丟 /AP 等於刪掉整幅畫，只能逐一改 path 顏色。
/// - **文字框 Stamp**：同理，文字是烘進外觀流的文字物件。
///
/// 這一版只處理標記類，其餘回明確錯誤而不是假裝成功——換色沒生效卻回 200
/// 是最糟的組合。
///
/// **已經試過且失敗的路，別再走一次**：拿 `annot.objects()`、對每個 path 物件
/// 呼叫 `set_stroke_color`、對文字物件呼叫 `set_fill_color`，然後存檔。編譯過、
/// 回 200，但**渲染出來的像素一個都沒變**（實測：手繪與文字框換成藍色後，紅色
/// 像素仍是 2769、藍色仍是 0）。原因是那些 setter 改的是 PDFium 記憶體裡的物件，
/// 而存檔寫回去的是註解原本的外觀流；pdfium-render 0.8 沒有公開「重建某個註解
/// 外觀流」的入口（`regenerate_content` 是**頁面**內容流，而且依 objects.rs 的
/// 註解，在移除物件後呼叫還會 double-free）。
///
/// 真要支援手繪／文字框換色，得從既有 path 物件把座標讀回來、刪掉整個註解、
/// 用新顏色重建並沿用原本的 /NM——那是另一塊工，且會碰到 objects.rs 記錄的
/// `remove_object` double-free 地雷。
pub fn set_color(path: &Path, page_index: u16, annot_id: &str, color: InColor) -> anyhow::Result<()> {
    use lopdf::Object;

    super::protect::assert_editable(path)?;
    let mut doc = lopdf::Document::load(path)?;
    let ids = normalize_page_annots(&mut doc, page_index)?;
    let target = find_by_nm(&doc, &ids, annot_id)
        .ok_or_else(|| anyhow::anyhow!("no annotation with id {annot_id} on this page"))?;

    let subtype = doc
        .get_dictionary(target)?
        .get(b"Subtype")
        .ok()
        .and_then(|o| o.as_name().ok())
        .map(|n| n.to_vec())
        .unwrap_or_default();
    if !matches!(
        subtype.as_slice(),
        b"Highlight" | b"Underline" | b"StrikeOut" | b"Squiggly"
    ) {
        anyhow::bail!(
            "cannot recolour this annotation type yet: its appearance is drawn, not generated from the colour entry"
        );
    }

    let dict = doc.get_dictionary_mut(target)?;
    dict.set(
        "C",
        Object::Array(vec![
            Object::Real(f32::from(color.r) / 255.0),
            Object::Real(f32::from(color.g) / 255.0),
            Object::Real(f32::from(color.b) / 255.0),
        ]),
    );
    // 外觀流是舊顏色的產物，留著的話 PDFium 會直接沿用、換色看起來完全沒發生。
    dict.remove(b"AP");
    dict.set("M", Object::string_literal(pdf_now()));
    save_in_place(&mut doc, path)
}

/// 移動／縮放註解（4c）。
///
/// 依 PDF 32000-1 §12.5.5，閱讀器把外觀流的 /BBox（經 /Matrix 變換後）映射進
/// 註解的 /Rect——所以對「外觀是畫出來的」那些類型（手繪、文字框、圖片印章），
/// 只要改 /Rect，內容就會跟著搬過去並依新尺寸縮放。這正好跟換色相反：換色
/// 對它們做不到，位移反而最單純。
///
/// 標記類（Highlight 等）不走這條：它們的幾何在 /QuadPoints，而且語意上是「貼在
/// 某段文字上」，硬拉伸並不合理。這一版直接拒絕，不假裝支援。
pub fn set_rect(
    pdfium: &Pdfium,
    path: &Path,
    page_index: u16,
    annot_id: &str,
    rect: InRect,
) -> anyhow::Result<()> {
    super::protect::assert_editable(path)?;
    with_document(pdfium, path, |doc| {
        let page = doc.pages().get(page_index)?;
        let page_height = page.height().value;
        let index = page
            .annotations()
            .iter()
            .position(|a| a.name().as_deref() == Some(annot_id))
            .ok_or_else(|| anyhow::anyhow!("no annotation with id {annot_id} on this page"))?;
        let mut annot = page.annotations().get(index as _)?;
        if !matches!(
            annot.annotation_type(),
            PdfPageAnnotationType::Ink
                | PdfPageAnnotationType::Stamp
                | PdfPageAnnotationType::Text
        ) {
            anyhow::bail!(
                "cannot move this annotation type: text markup is anchored to the text it covers, not to a free rectangle"
            );
        }
        annot.set_bounds(to_pdf_rect(&rect, page_height))?;
        Ok(())
    })
}
