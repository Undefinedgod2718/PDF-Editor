//! AcroForm field creation / update / deletion (P14).
//!
//! pdfium-render (and FPDF itself) cannot create form fields, so everything
//! here is lopdf dictionary surgery, following the same pattern as
//! `formops::set_button_lopdf`: load with lopdf, mutate, atomic tmp+rename
//! save. Every entry point calls `protect::assert_editable` first — an
//! lopdf load+save of an empty-user-password document would silently strip
//! `/Encrypt` (see protect.rs).
//!
//! Coordinates crossing the API are PDF points with a top-left origin
//! (project-wide convention, see annots.rs); they are flipped to PDF's
//! bottom-left origin against the page MediaBox here. Pages whose CropBox
//! differs from MediaBox may show a placement offset (known v1 trade-off,
//! documented in P14-Review).

use std::path::Path;

use lopdf::{dictionary, Dictionary, Document, Object, ObjectId, Stream};
use serde::Deserialize;

use super::annots::InRect;
use super::protect;

/// Minimum widget edge in points; smaller rects are always a mis-drag.
const MIN_SIZE: f32 = 8.0;

// Field flag bits (PDF 32000-1 tables 226/227/229/230).
const FF_REQUIRED: i64 = 1 << 1;
const FF_MULTILINE: i64 = 1 << 12;
const FF_NO_TOGGLE_TO_OFF: i64 = 1 << 14;
const FF_RADIO: i64 = 1 << 15;
const FF_COMBO: i64 = 1 << 17;

#[derive(Deserialize)]
pub struct RadioOption {
    pub value: String,
    pub rect: InRect,
}

#[derive(Deserialize)]
#[serde(tag = "fieldType", rename_all = "camelCase")]
pub enum NewField {
    #[serde(rename_all = "camelCase")]
    Text {
        name: String,
        rect: InRect,
        #[serde(default)]
        multiline: bool,
        #[serde(default)]
        required: bool,
        #[serde(default = "default_font_size")]
        font_size: f32,
        #[serde(default)]
        default_value: Option<String>,
    },
    Checkbox {
        name: String,
        rect: InRect,
        #[serde(default)]
        required: bool,
    },
    Radio {
        name: String,
        options: Vec<RadioOption>,
        #[serde(default)]
        required: bool,
    },
    Combobox {
        name: String,
        rect: InRect,
        options: Vec<String>,
        #[serde(default)]
        required: bool,
    },
    Listbox {
        name: String,
        rect: InRect,
        options: Vec<String>,
        #[serde(default)]
        required: bool,
    },
    Signature { name: String, rect: InRect },
}

fn default_font_size() -> f32 {
    12.0
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FieldUpdate {
    pub rect: Option<InRect>,
    pub name: Option<String>,
    pub options: Option<Vec<String>>,
    pub required: Option<bool>,
}

/// Caller-fixable (400) vs internal (500), mirroring `protect::ProtectError`.
pub enum FormBuildError {
    User(String),
    Internal(anyhow::Error),
}

impl From<anyhow::Error> for FormBuildError {
    fn from(e: anyhow::Error) -> Self {
        FormBuildError::Internal(e)
    }
}

impl From<lopdf::Error> for FormBuildError {
    fn from(e: lopdf::Error) -> Self {
        FormBuildError::Internal(e.into())
    }
}

fn user_err<T>(msg: impl Into<String>) -> Result<T, FormBuildError> {
    Err(FormBuildError::User(msg.into()))
}

// ---------------------------------------------------------------------------
// Entry points
// ---------------------------------------------------------------------------

pub fn create_field(
    path: &Path,
    page_index: u16,
    field: &NewField,
) -> Result<(), FormBuildError> {
    protect::assert_editable(path).map_err(|e| FormBuildError::User(e.to_string()))?;
    let mut doc = Document::load(path).map_err(anyhow::Error::from)?;

    let name = field_name(field);
    validate_name(&doc, name, None)?;

    let page_id = page_id(&doc, page_index)?;
    let media = media_box(&doc, page_id)?;

    match field {
        NewField::Radio { name, options, required } => {
            if options.len() < 2 {
                return user_err("radio group needs at least 2 options");
            }
            if options.iter().any(|o| o.value.trim().is_empty()) {
                return user_err("radio option value must not be empty");
            }
            let rects: Vec<[f32; 4]> = options
                .iter()
                .map(|o| flip_rect(&o.rect, &media))
                .collect::<Result<_, _>>()?;
            // flip_rect clamps each option independently; if the client sent
            // options past the page bottom they all collapse onto the same Y
            // and overlap. Refuse that instead of writing a broken group.
            for pair in rects.windows(2) {
                let gap = (pair[0][1] - pair[1][1]).abs();
                if gap < 1.0 {
                    return user_err(
                        "radio options overlap or extend past the page; shrink the field or use fewer options",
                    );
                }
            }
            create_radio_group(&mut doc, page_id, name, options, &rects, *required)?;
        }
        _ => {
            let rect = flip_rect(single_rect(field), &media)?;
            create_single(&mut doc, page_id, field, rect)?;
        }
    }

    save_atomic(&mut doc, path)?;
    Ok(())
}

pub fn update_field(
    path: &Path,
    page_index: u16,
    annot_index: usize,
    upd: &FieldUpdate,
) -> Result<(), FormBuildError> {
    if upd.rect.is_none() && upd.name.is_none() && upd.options.is_none() && upd.required.is_none()
    {
        return user_err("update needs at least one of rect/name/options/required");
    }
    protect::assert_editable(path).map_err(|e| FormBuildError::User(e.to_string()))?;
    let mut doc = Document::load(path).map_err(anyhow::Error::from)?;

    let page_id = page_id(&doc, page_index)?;
    let annot_id = annot_at(&doc, page_id, annot_index)?;
    // The dictionary that owns /T, /Ff, /Opt: the parent for a radio kid,
    // the widget itself for merged field+widget dictionaries.
    let field_id = parent_of(&doc, annot_id).unwrap_or(annot_id);

    if let Some(name) = &upd.name {
        validate_name(&doc, name, Some(field_id))?;
        let t = pdf_text_string(name);
        doc.get_dictionary_mut(field_id)?.set("T", t);
    }

    if let Some(required) = upd.required {
        let dict = doc.get_dictionary_mut(field_id)?;
        let ff = dict.get(b"Ff").and_then(|o| o.as_i64()).unwrap_or(0);
        let ff = if required { ff | FF_REQUIRED } else { ff & !FF_REQUIRED };
        dict.set("Ff", ff);
    }

    if let Some(options) = &upd.options {
        let ft = field_type(&doc, field_id);
        if ft.as_deref() != Some(b"Ch") {
            return user_err("options can only be updated on combobox/listbox fields");
        }
        if options.is_empty() {
            return user_err("choice field needs at least 1 option");
        }
        let opt: Vec<Object> = options.iter().map(|s| pdf_text_string(s)).collect();
        doc.get_dictionary_mut(field_id)?.set("Opt", opt);
    }

    if let Some(rect) = &upd.rect {
        let media = media_box(&doc, page_id)?;
        let r = flip_rect(rect, &media)?;
        let (w, h) = (r[2] - r[0], r[3] - r[1]);
        // The appearance stream's BBox no longer matches → regenerate.
        let kind = appearance_kind(&doc, annot_id, field_id);
        let on_state = existing_on_state(&doc, annot_id);
        let zadb = ensure_acroform(&mut doc)?.zadb;
        let ap = build_appearance(&mut doc, kind, w, h, zadb, on_state);
        let annot = doc.get_dictionary_mut(annot_id)?;
        annot.set("Rect", rect_array(r));
        annot.set("AP", ap);
    }

    save_atomic(&mut doc, path)?;
    Ok(())
}

pub fn delete_field(
    path: &Path,
    page_index: u16,
    annot_index: usize,
) -> Result<(), FormBuildError> {
    protect::assert_editable(path).map_err(|e| FormBuildError::User(e.to_string()))?;
    let mut doc = Document::load(path).map_err(anyhow::Error::from)?;

    let page_id = page_id(&doc, page_index)?;
    let annot_id = annot_at(&doc, page_id, annot_index)?;

    // Drop the widget from the page's /Annots.
    let mut annots = resolve_array(&doc, doc.get_dictionary(page_id)?.get(b"Annots")?)
        .ok_or_else(|| anyhow::anyhow!("page /Annots is not an array"))?;
    annots.retain(|o| o.as_reference().ok() != Some(annot_id));
    doc.get_dictionary_mut(page_id)?.set("Annots", annots);

    match parent_of(&doc, annot_id) {
        Some(parent_id) => {
            // Radio kid (or any child widget): remove from /Kids; drop the
            // parent from /Fields when the last kid goes.
            let parent = doc.get_dictionary(parent_id)?;
            let mut kids = parent
                .get(b"Kids")
                .ok()
                .and_then(|k| resolve_array(&doc, k))
                .unwrap_or_default();
            kids.retain(|o| o.as_reference().ok() != Some(annot_id));
            let empty = kids.is_empty();
            doc.get_dictionary_mut(parent_id)?.set("Kids", kids);
            if empty {
                remove_from_fields(&mut doc, parent_id)?;
            }
        }
        None => remove_from_fields(&mut doc, annot_id)?,
    }

    save_atomic(&mut doc, path)?;
    Ok(())
}

// ---------------------------------------------------------------------------
// Creation internals
// ---------------------------------------------------------------------------

struct AcroFormRefs {
    form_id: ObjectId,
    zadb: ObjectId,
}

fn create_single(
    doc: &mut Document,
    page_id: ObjectId,
    field: &NewField,
    rect: [f32; 4],
) -> Result<(), FormBuildError> {
    let acro = ensure_acroform(doc)?;
    let (w, h) = (rect[2] - rect[0], rect[3] - rect[1]);

    let mut dict = dictionary! {
        "Type" => "Annot",
        "Subtype" => "Widget",
        "Rect" => rect_array(rect),
        "F" => 4,
        "P" => Object::Reference(page_id),
        "MK" => dictionary! {
            "BC" => vec![0.into(), 0.into(), 0.into()],
            "BG" => vec![1.into(), 1.into(), 1.into()],
        },
    };

    match field {
        NewField::Text { name, multiline, required, font_size, default_value, .. } => {
            dict.set("FT", "Tx");
            dict.set("T", pdf_text_string(name));
            let mut ff = 0i64;
            if *multiline {
                ff |= FF_MULTILINE;
            }
            if *required {
                ff |= FF_REQUIRED;
            }
            dict.set("Ff", ff);
            dict.set(
                "DA",
                Object::string_literal(format!("/Helv {} Tf 0 g", font_size)),
            );
            if let Some(v) = default_value {
                dict.set("V", pdf_text_string(v));
                dict.set("DV", pdf_text_string(v));
            }
        }
        NewField::Checkbox { name, required, .. } => {
            dict.set("FT", "Btn");
            dict.set("T", pdf_text_string(name));
            dict.set("Ff", if *required { FF_REQUIRED } else { 0 });
            dict.set("DA", Object::string_literal("/ZaDb 0 Tf 0 g"));
            dict.set("V", Object::Name(b"Off".to_vec()));
            dict.set("AS", Object::Name(b"Off".to_vec()));
        }
        NewField::Combobox { name, options, required, .. }
        | NewField::Listbox { name, options, required, .. } => {
            if options.is_empty() {
                return user_err("choice field needs at least 1 option");
            }
            if options.iter().any(|s| s.trim().is_empty()) {
                return user_err("choice option must not be empty");
            }
            dict.set("FT", "Ch");
            dict.set("T", pdf_text_string(name));
            let mut ff = if matches!(field, NewField::Combobox { .. }) { FF_COMBO } else { 0 };
            if *required {
                ff |= FF_REQUIRED;
            }
            dict.set("Ff", ff);
            dict.set("DA", Object::string_literal("/Helv 12 Tf 0 g"));
            let opt: Vec<Object> = options.iter().map(|s| pdf_text_string(s)).collect();
            dict.set("Opt", opt);
        }
        NewField::Signature { name, .. } => {
            dict.set("FT", "Sig");
            dict.set("T", pdf_text_string(name));
        }
        NewField::Radio { .. } => unreachable!("radio handled by create_radio_group"),
    }

    let kind = match field {
        NewField::Checkbox { .. } => ApKind::Check,
        _ => ApKind::Box,
    };
    let widget_id = doc.add_object(dict);
    let ap = build_appearance(doc, kind, w, h, acro.zadb, None);
    doc.get_dictionary_mut(widget_id)?.set("AP", ap);

    push_page_annot(doc, page_id, widget_id)?;
    push_field(doc, acro.form_id, widget_id)?;
    Ok(())
}

fn create_radio_group(
    doc: &mut Document,
    page_id: ObjectId,
    name: &str,
    options: &[RadioOption],
    rects: &[[f32; 4]],
    required: bool,
) -> Result<(), FormBuildError> {
    let acro = ensure_acroform(doc)?;

    let mut ff = FF_RADIO | FF_NO_TOGGLE_TO_OFF;
    if required {
        ff |= FF_REQUIRED;
    }
    // /Opt keeps the human-readable option values; kid on-state names are
    // synthetic ASCII (`Opt0`, `Opt1`, …) so CJK values never end up in a
    // PDF name object.
    let opt: Vec<Object> = options.iter().map(|o| pdf_text_string(&o.value)).collect();
    let parent_id = doc.add_object(dictionary! {
        "FT" => "Btn",
        "T" => pdf_text_string(name),
        "Ff" => ff,
        "V" => Object::Name(b"Off".to_vec()),
        "DV" => Object::Name(b"Off".to_vec()),
        "Opt" => opt,
        "Kids" => Vec::<Object>::new(),
    });

    let mut kid_refs: Vec<Object> = Vec::new();
    for (i, rect) in rects.iter().enumerate() {
        let (w, h) = (rect[2] - rect[0], rect[3] - rect[1]);
        let on_name = format!("Opt{i}");
        let kid = dictionary! {
            "Type" => "Annot",
            "Subtype" => "Widget",
            "Rect" => rect_array(*rect),
            "F" => 4,
            "P" => Object::Reference(page_id),
            "Parent" => Object::Reference(parent_id),
            "AS" => Object::Name(b"Off".to_vec()),
            "MK" => dictionary! {
                "BC" => vec![0.into(), 0.into(), 0.into()],
                "BG" => vec![1.into(), 1.into(), 1.into()],
            },
        };
        let kid_id = doc.add_object(kid);
        let ap = build_appearance(doc, ApKind::Radio, w, h, acro.zadb, Some(on_name.into_bytes()));
        doc.get_dictionary_mut(kid_id)?.set("AP", ap);
        push_page_annot(doc, page_id, kid_id)?;
        kid_refs.push(Object::Reference(kid_id));
    }

    doc.get_dictionary_mut(parent_id)?.set("Kids", kid_refs);
    push_field(doc, acro.form_id, parent_id)?;
    Ok(())
}

// ---------------------------------------------------------------------------
// AcroForm plumbing
// ---------------------------------------------------------------------------

/// Make sure `/Root /AcroForm` exists as an indirect object with /Fields,
/// /NeedAppearances, /DA and a /DR carrying Helv + ZaDb. Existing keys are
/// left untouched. Returns the AcroForm id plus the ZaDb font id (needed
/// for appearance-stream resources).
fn ensure_acroform(doc: &mut Document) -> Result<AcroFormRefs, FormBuildError> {
    let root_id = doc
        .trailer
        .get(b"Root")
        .and_then(|o| o.as_reference())
        .map_err(|_| anyhow::anyhow!("trailer /Root missing"))?;

    // Normalize AcroForm to an indirect object.
    let form_id = match doc.get_dictionary(root_id)?.get(b"AcroForm") {
        Ok(Object::Reference(id)) => *id,
        Ok(Object::Dictionary(d)) => {
            let d = d.clone();
            let id = doc.add_object(d);
            doc.get_dictionary_mut(root_id)?
                .set("AcroForm", Object::Reference(id));
            id
        }
        _ => {
            let id = doc.add_object(Dictionary::new());
            doc.get_dictionary_mut(root_id)?
                .set("AcroForm", Object::Reference(id));
            id
        }
    };

    // /DR /Font: reuse present Helv/ZaDb, create missing ones.
    let existing = |doc: &Document, key: &[u8]| -> Option<ObjectId> {
        let form = doc.get_dictionary(form_id).ok()?;
        let dr = resolve_dict(doc, form.get(b"DR").ok()?)?;
        let font = resolve_dict(doc, dr.get(b"Font").ok()?)?;
        font.get(key).ok()?.as_reference().ok()
    };
    let helv = match existing(doc, b"Helv") {
        Some(id) => id,
        None => doc.add_object(dictionary! {
            "Type" => "Font", "Subtype" => "Type1", "BaseFont" => "Helvetica",
        }),
    };
    let zadb = match existing(doc, b"ZaDb") {
        Some(id) => id,
        None => doc.add_object(dictionary! {
            "Type" => "Font", "Subtype" => "Type1", "BaseFont" => "ZapfDingbats",
        }),
    };

    // Re-read + rewrite DR as a direct dictionary on the form.
    let form = doc.get_dictionary(form_id)?;
    let mut dr = form
        .get(b"DR")
        .ok()
        .and_then(|o| resolve_dict(doc, o))
        .unwrap_or_default();
    let mut font = dr
        .get(b"Font")
        .ok()
        .and_then(|o| resolve_dict(doc, o))
        .unwrap_or_default();
    font.set("Helv", Object::Reference(helv));
    font.set("ZaDb", Object::Reference(zadb));
    dr.set("Font", font);

    let has_fields = form.has(b"Fields");
    let has_da = form.has(b"DA");

    let form_mut = doc.get_dictionary_mut(form_id)?;
    form_mut.set("DR", dr);
    form_mut.set("NeedAppearances", true);
    if !has_fields {
        form_mut.set("Fields", Vec::<Object>::new());
    }
    if !has_da {
        form_mut.set("DA", Object::string_literal("/Helv 0 Tf 0 g"));
    }

    Ok(AcroFormRefs { form_id, zadb })
}

fn push_field(
    doc: &mut Document,
    form_id: ObjectId,
    field_id: ObjectId,
) -> Result<(), FormBuildError> {
    let form = doc.get_dictionary(form_id)?;
    let mut fields = form
        .get(b"Fields")
        .ok()
        .and_then(|f| resolve_array(doc, f))
        .unwrap_or_default();
    fields.push(Object::Reference(field_id));
    doc.get_dictionary_mut(form_id)?.set("Fields", fields);
    Ok(())
}

fn remove_from_fields(doc: &mut Document, field_id: ObjectId) -> Result<(), FormBuildError> {
    let root_id = doc
        .trailer
        .get(b"Root")
        .and_then(|o| o.as_reference())
        .map_err(|_| anyhow::anyhow!("trailer /Root missing"))?;
    let Some(form_id) = doc
        .get_dictionary(root_id)?
        .get(b"AcroForm")
        .ok()
        .and_then(|o| o.as_reference().ok())
    else {
        return Ok(()); // no AcroForm — nothing to clean up
    };
    let form = doc.get_dictionary(form_id)?;
    let mut fields = form
        .get(b"Fields")
        .ok()
        .and_then(|f| resolve_array(doc, f))
        .unwrap_or_default();
    fields.retain(|o| o.as_reference().ok() != Some(field_id));
    doc.get_dictionary_mut(form_id)?.set("Fields", fields);
    Ok(())
}

fn push_page_annot(
    doc: &mut Document,
    page_id: ObjectId,
    annot_id: ObjectId,
) -> Result<(), FormBuildError> {
    let page = doc.get_dictionary(page_id)?;
    let mut annots = page
        .get(b"Annots")
        .ok()
        .and_then(|a| resolve_array(doc, a))
        .unwrap_or_default();
    annots.push(Object::Reference(annot_id));
    doc.get_dictionary_mut(page_id)?.set("Annots", annots);
    Ok(())
}

// ---------------------------------------------------------------------------
// Appearance streams
// ---------------------------------------------------------------------------

#[derive(Clone, Copy)]
enum ApKind {
    /// White fill, black border — text/choice/signature and all off-states.
    Box,
    /// Box + ZapfDingbats check mark (char `4`) for the on-state.
    Check,
    /// Box + ZapfDingbats filled circle (char `l`) for the on-state.
    Radio,
}

/// Build the /AP dictionary for a widget of `kind` and size `w`×`h`.
/// For buttons, `on_state` names the on appearance (defaults to `Yes`).
fn build_appearance(
    doc: &mut Document,
    kind: ApKind,
    w: f32,
    h: f32,
    zadb: ObjectId,
    on_state: Option<Vec<u8>>,
) -> Dictionary {
    let box_content = format!(
        "q 1 1 1 rg 0 0 {w:.2} {h:.2} re f 0 0 0 RG 1 w 0.5 0.5 {:.2} {:.2} re S Q",
        w - 1.0,
        h - 1.0
    );
    match kind {
        ApKind::Box => {
            let n = form_xobject(doc, w, h, &box_content, None);
            dictionary! { "N" => Object::Reference(n) }
        }
        ApKind::Check | ApKind::Radio => {
            let glyph = if matches!(kind, ApKind::Check) { "4" } else { "l" };
            let size = 0.8 * w.min(h);
            // Rough centring; ZapfDingbats glyphs sit near the em centre.
            let tx = w / 2.0 - size * 0.35;
            let ty = h / 2.0 - size * 0.35;
            let on_content = format!(
                "{box_content} q BT /ZaDb {size:.2} Tf 0 g {tx:.2} {ty:.2} Td ({glyph}) Tj ET Q"
            );
            let off = form_xobject(doc, w, h, &box_content, None);
            let on = form_xobject(doc, w, h, &on_content, Some(zadb));
            let on_name = on_state.unwrap_or_else(|| b"Yes".to_vec());
            let mut n = Dictionary::new();
            n.set(on_name, Object::Reference(on));
            n.set("Off", Object::Reference(off));
            dictionary! { "N" => n }
        }
    }
}

fn form_xobject(
    doc: &mut Document,
    w: f32,
    h: f32,
    content: &str,
    zadb: Option<ObjectId>,
) -> ObjectId {
    let mut dict = dictionary! {
        "Type" => "XObject",
        "Subtype" => "Form",
        "BBox" => vec![0.into(), 0.into(), w.into(), h.into()],
    };
    if let Some(zadb) = zadb {
        dict.set(
            "Resources",
            dictionary! { "Font" => dictionary! { "ZaDb" => Object::Reference(zadb) } },
        );
    }
    doc.add_object(Stream::new(dict, content.as_bytes().to_vec()))
}

/// The AP kind to regenerate on resize, inferred from the field type.
fn appearance_kind(doc: &Document, annot_id: ObjectId, field_id: ObjectId) -> ApKind {
    match field_type(doc, field_id).as_deref() {
        Some(b"Btn") => {
            let radio = doc
                .get_dictionary(field_id)
                .ok()
                .and_then(|d| d.get(b"Ff").ok().and_then(|o| o.as_i64().ok()))
                .map(|ff| ff & FF_RADIO != 0)
                .unwrap_or(false);
            // A kid widget's field dict is the parent; merged checkbox is itself.
            let _ = annot_id;
            if radio {
                ApKind::Radio
            } else {
                ApKind::Check
            }
        }
        _ => ApKind::Box,
    }
}

/// Preserve the current on-state name (`/AP /N` key ≠ Off) across an AP
/// rebuild so `/V`//`/AS` values keep matching.
fn existing_on_state(doc: &Document, annot_id: ObjectId) -> Option<Vec<u8>> {
    let annot = doc.get_dictionary(annot_id).ok()?;
    let ap = resolve_dict(doc, annot.get(b"AP").ok()?)?;
    let n = resolve_dict(doc, ap.get(b"N").ok()?)?;
    n.iter()
        .map(|(k, _)| k.clone())
        .find(|k| k.as_slice() != b"Off")
}

// ---------------------------------------------------------------------------
// Addressing / geometry / naming helpers
// ---------------------------------------------------------------------------

fn page_id(doc: &Document, page_index: u16) -> Result<ObjectId, FormBuildError> {
    doc.get_pages()
        .get(&(page_index as u32 + 1))
        .copied()
        .ok_or_else(|| FormBuildError::User(format!("page {page_index} out of range")))
}

fn annot_at(
    doc: &Document,
    page_id: ObjectId,
    annot_index: usize,
) -> Result<ObjectId, FormBuildError> {
    let page = doc.get_dictionary(page_id)?;
    let annots = page
        .get(b"Annots")
        .ok()
        .and_then(|a| resolve_array(doc, a))
        .ok_or_else(|| FormBuildError::User("page has no annotations".into()))?;
    let id = annots
        .get(annot_index)
        .and_then(|o| o.as_reference().ok())
        .ok_or_else(|| {
            FormBuildError::User(format!("annotation index {annot_index} out of range"))
        })?;
    let dict = doc.get_dictionary(id)?;
    let is_widget = dict
        .get(b"Subtype")
        .and_then(|o| o.as_name())
        .map(|n| n == b"Widget")
        .unwrap_or(false);
    if !is_widget {
        return user_err(format!("annotation {annot_index} is not a form field widget"));
    }
    Ok(id)
}

fn parent_of(doc: &Document, annot_id: ObjectId) -> Option<ObjectId> {
    doc.get_dictionary(annot_id)
        .ok()?
        .get(b"Parent")
        .ok()?
        .as_reference()
        .ok()
}

fn field_type(doc: &Document, field_id: ObjectId) -> Option<Vec<u8>> {
    doc.get_dictionary(field_id)
        .ok()?
        .get(b"FT")
        .ok()?
        .as_name()
        .ok()
        .map(|n| n.to_vec())
}

/// MediaBox resolved through page-tree inheritance, normalized to
/// `[x0, y0, x1, y1]` with x0<x1, y0<y1.
fn media_box(doc: &Document, page_id: ObjectId) -> Result<[f32; 4], FormBuildError> {
    let mut current = Some(page_id);
    while let Some(id) = current {
        let dict = doc.get_dictionary(id)?;
        if let Some(arr) = dict.get(b"MediaBox").ok().and_then(|o| resolve_array(doc, o)) {
            let v: Vec<f32> = arr
                .iter()
                .filter_map(|o| match o {
                    Object::Integer(i) => Some(*i as f32),
                    Object::Real(r) => Some(*r),
                    _ => None,
                })
                .collect();
            if v.len() == 4 {
                return Ok([
                    v[0].min(v[2]),
                    v[1].min(v[3]),
                    v[0].max(v[2]),
                    v[1].max(v[3]),
                ]);
            }
            return Err(anyhow::anyhow!("malformed /MediaBox").into());
        }
        current = dict.get(b"Parent").ok().and_then(|p| p.as_reference().ok());
    }
    Err(anyhow::anyhow!("page has no /MediaBox").into())
}

/// Top-left-origin API rect → PDF `[x0, y0, x1, y1]` in MediaBox space.
fn flip_rect(rect: &InRect, media: &[f32; 4]) -> Result<[f32; 4], FormBuildError> {
    let (page_w, page_h) = (media[2] - media[0], media[3] - media[1]);
    if !(rect.w.is_finite() && rect.h.is_finite() && rect.x.is_finite() && rect.y.is_finite()) {
        return user_err("rect must be finite");
    }
    if rect.w < MIN_SIZE || rect.h < MIN_SIZE {
        return user_err(format!("field must be at least {MIN_SIZE}pt on each side"));
    }
    if rect.w > page_w || rect.h > page_h {
        return user_err("field is larger than the page");
    }
    let x = rect.x.clamp(0.0, page_w - rect.w);
    let y = rect.y.clamp(0.0, page_h - rect.h);
    let x0 = media[0] + x;
    let y0 = media[3] - y - rect.h;
    Ok([x0, y0, x0 + rect.w, y0 + rect.h])
}

fn rect_array(r: [f32; 4]) -> Vec<Object> {
    r.iter().map(|v| Object::Real(*v)).collect()
}

fn field_name(field: &NewField) -> &str {
    match field {
        NewField::Text { name, .. }
        | NewField::Checkbox { name, .. }
        | NewField::Radio { name, .. }
        | NewField::Combobox { name, .. }
        | NewField::Listbox { name, .. }
        | NewField::Signature { name, .. } => name,
    }
}

/// Non-empty, no `.` (partial-name separator), unique across every /T in
/// the field tree. `exclude` skips one field id (renaming to itself).
fn validate_name(
    doc: &Document,
    name: &str,
    exclude: Option<ObjectId>,
) -> Result<(), FormBuildError> {
    let trimmed = name.trim();
    if trimmed.is_empty() {
        return user_err("field name must not be empty");
    }
    if trimmed != name {
        return user_err("field name must not have leading/trailing whitespace");
    }
    if name.contains('.') {
        return user_err("field name must not contain '.' (partial-name separator)");
    }
    let mut names = Vec::new();
    collect_field_names(doc, exclude, &mut names);
    if names.iter().any(|n| n == name) {
        return user_err(format!("a field named \"{name}\" already exists"));
    }
    Ok(())
}

fn collect_field_names(doc: &Document, exclude: Option<ObjectId>, out: &mut Vec<String>) {
    let Ok(root_id) = doc.trailer.get(b"Root").and_then(|o| o.as_reference()) else {
        return;
    };
    let Some(form) = doc
        .get_dictionary(root_id)
        .ok()
        .and_then(|r| r.get(b"AcroForm").ok().and_then(|o| resolve_dict(doc, o)))
    else {
        return;
    };
    let Some(fields) = form.get(b"Fields").ok().and_then(|f| resolve_array(doc, f)) else {
        return;
    };
    fn walk(doc: &Document, obj: &Object, exclude: Option<ObjectId>, out: &mut Vec<String>) {
        let (id, dict) = match obj {
            Object::Reference(id) => match doc.get_dictionary(*id) {
                Ok(d) => (Some(*id), d.clone()),
                Err(_) => return,
            },
            Object::Dictionary(d) => (None, d.clone()),
            _ => return,
        };
        if id.is_none() || id != exclude {
            if let Ok(t) = dict.get(b"T") {
                if let Some(s) = decode_text_string(doc, t) {
                    out.push(s);
                }
            }
        }
        if let Some(kids) = dict.get(b"Kids").ok().and_then(|k| resolve_array(doc, k)) {
            for kid in &kids {
                walk(doc, kid, exclude, out);
            }
        }
    }
    for f in &fields {
        walk(doc, f, exclude, out);
    }
}

// ---------------------------------------------------------------------------
// PDF text strings
// ---------------------------------------------------------------------------

/// Encode per PDF 32000-1 §7.9.2.2: plain literal when ASCII, otherwise
/// UTF-16BE with BOM. Never write raw UTF-8 into a text string.
fn pdf_text_string(s: &str) -> Object {
    if s.is_ascii() {
        return Object::string_literal(s);
    }
    let mut bytes = vec![0xFE, 0xFF];
    for unit in s.encode_utf16() {
        bytes.extend_from_slice(&unit.to_be_bytes());
    }
    Object::String(bytes, lopdf::StringFormat::Hexadecimal)
}

fn decode_text_string(doc: &Document, obj: &Object) -> Option<String> {
    let obj = match obj {
        Object::Reference(id) => doc.get_object(*id).ok()?,
        other => other,
    };
    let Object::String(bytes, _) = obj else {
        return None;
    };
    if bytes.starts_with(&[0xFE, 0xFF]) {
        let units: Vec<u16> = bytes[2..]
            .chunks_exact(2)
            .map(|c| u16::from_be_bytes([c[0], c[1]]))
            .collect();
        return String::from_utf16(&units).ok();
    }
    Some(String::from_utf8_lossy(bytes).into_owned())
}

// ---------------------------------------------------------------------------
// Shared low-level helpers
// ---------------------------------------------------------------------------

fn resolve_array(doc: &Document, obj: &Object) -> Option<Vec<Object>> {
    match obj {
        Object::Array(a) => Some(a.clone()),
        Object::Reference(id) => doc
            .get_object(*id)
            .ok()
            .and_then(|o| o.as_array().ok().cloned()),
        _ => None,
    }
}

fn resolve_dict(doc: &Document, obj: &Object) -> Option<Dictionary> {
    match obj {
        Object::Dictionary(d) => Some(d.clone()),
        Object::Reference(id) => doc.get_dictionary(*id).ok().cloned(),
        _ => None,
    }
}

fn single_rect(field: &NewField) -> &InRect {
    match field {
        NewField::Text { rect, .. }
        | NewField::Checkbox { rect, .. }
        | NewField::Combobox { rect, .. }
        | NewField::Listbox { rect, .. }
        | NewField::Signature { rect, .. } => rect,
        NewField::Radio { .. } => unreachable!("radio has per-option rects"),
    }
}

fn save_atomic(doc: &mut Document, path: &Path) -> Result<(), FormBuildError> {
    let mut bytes = Vec::new();
    doc.save_to(&mut bytes).map_err(anyhow::Error::from)?;
    let tmp = path.with_extension("pdf.tmp");
    std::fs::write(&tmp, &bytes).map_err(anyhow::Error::from)?;
    std::fs::rename(&tmp, path).map_err(anyhow::Error::from)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Minimal one-page 612×792 PDF written to a unique temp file.
    fn temp_pdf() -> std::path::PathBuf {
        let mut doc = Document::with_version("1.5");
        let pages_id = doc.new_object_id();
        let page_id = doc.add_object(dictionary! {
            "Type" => "Page",
            "Parent" => Object::Reference(pages_id),
            "MediaBox" => vec![0.into(), 0.into(), 612.into(), 792.into()],
        });
        doc.objects.insert(
            pages_id,
            Object::Dictionary(dictionary! {
                "Type" => "Pages",
                "Kids" => vec![Object::Reference(page_id)],
                "Count" => 1,
            }),
        );
        let catalog = doc.add_object(dictionary! {
            "Type" => "Catalog",
            "Pages" => Object::Reference(pages_id),
        });
        doc.trailer.set("Root", Object::Reference(catalog));
        let path =
            std::env::temp_dir().join(format!("formbuild_test_{}.pdf", uuid::Uuid::new_v4()));
        doc.save(&path).unwrap();
        path
    }

    fn text_field(name: &str) -> NewField {
        NewField::Text {
            name: name.into(),
            rect: InRect { x: 72.0, y: 100.0, w: 180.0, h: 24.0 },
            multiline: false,
            required: false,
            font_size: 12.0,
            default_value: None,
        }
    }

    fn acroform(doc: &Document) -> Dictionary {
        let root = doc.trailer.get(b"Root").and_then(|o| o.as_reference()).unwrap();
        let form = doc.get_dictionary(root).unwrap().get(b"AcroForm").unwrap();
        resolve_dict(doc, form).unwrap()
    }

    fn fields(doc: &Document) -> Vec<Object> {
        resolve_array(doc, acroform(doc).get(b"Fields").unwrap()).unwrap()
    }

    fn page_annots(doc: &Document) -> Vec<Object> {
        let page_id = *doc.get_pages().get(&1).unwrap();
        doc.get_dictionary(page_id)
            .unwrap()
            .get(b"Annots")
            .ok()
            .and_then(|a| resolve_array(doc, a))
            .unwrap_or_default()
    }

    fn err_str(e: FormBuildError) -> String {
        match e {
            FormBuildError::User(m) => m,
            FormBuildError::Internal(e) => format!("internal: {e}"),
        }
    }

    #[test]
    fn text_field_roundtrip() {
        let path = temp_pdf();
        create_field(&path, 0, &text_field("name1")).map_err(err_str).unwrap();

        let doc = Document::load(&path).unwrap();
        let form = acroform(&doc);
        assert!(form.get(b"NeedAppearances").unwrap().as_bool().unwrap());
        let fs = fields(&doc);
        assert_eq!(fs.len(), 1);
        let widget = doc.get_dictionary(fs[0].as_reference().unwrap()).unwrap();
        assert_eq!(widget.get(b"FT").unwrap().as_name().unwrap(), b"Tx");
        // top-left (72,100) h=24 on a 792pt page: bottom y0 = 792-100-24 = 668
        let rect = resolve_array(&doc, widget.get(b"Rect").unwrap()).unwrap();
        let y0 = match rect[1] {
            Object::Real(r) => r,
            Object::Integer(i) => i as f32,
            _ => panic!("rect y0 not a number"),
        };
        assert!((y0 - 668.0).abs() < 0.01, "y0 = {y0}");
        assert!(widget.get(b"AP").is_ok());
        assert_eq!(page_annots(&doc).len(), 1);
        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn duplicate_name_rejected() {
        let path = temp_pdf();
        create_field(&path, 0, &text_field("dup")).map_err(err_str).unwrap();
        let err = create_field(&path, 0, &text_field("dup")).map_err(err_str).unwrap_err();
        assert!(err.contains("already exists"), "unexpected: {err}");
        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn dotted_and_empty_names_rejected() {
        let path = temp_pdf();
        for bad in ["", "  ", "a.b"] {
            let err = create_field(&path, 0, &text_field(bad)).map_err(err_str).unwrap_err();
            assert!(err.contains("name"), "unexpected: {err}");
        }
        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn radio_group_structure() {
        let path = temp_pdf();
        let field = NewField::Radio {
            name: "性別".into(),
            required: false,
            options: vec![
                RadioOption {
                    value: "男".into(),
                    rect: InRect { x: 72.0, y: 100.0, w: 16.0, h: 16.0 },
                },
                RadioOption {
                    value: "女".into(),
                    rect: InRect { x: 72.0, y: 130.0, w: 16.0, h: 16.0 },
                },
            ],
        };
        create_field(&path, 0, &field).map_err(err_str).unwrap();

        let doc = Document::load(&path).unwrap();
        let fs = fields(&doc);
        assert_eq!(fs.len(), 1);
        let parent_id = fs[0].as_reference().unwrap();
        let parent = doc.get_dictionary(parent_id).unwrap();
        let ff = parent.get(b"Ff").unwrap().as_i64().unwrap();
        assert!(ff & FF_RADIO != 0);
        // CJK group name must be UTF-16BE with BOM
        let t = match parent.get(b"T").unwrap() {
            Object::String(b, _) => b.clone(),
            _ => panic!("/T not a string"),
        };
        assert_eq!(&t[..2], &[0xFE, 0xFF]);
        let kids = resolve_array(&doc, parent.get(b"Kids").unwrap()).unwrap();
        assert_eq!(kids.len(), 2);
        assert_eq!(page_annots(&doc).len(), 2);
        // kid on-state is synthetic ASCII, off state present
        let kid = doc.get_dictionary(kids[0].as_reference().unwrap()).unwrap();
        let ap = resolve_dict(&doc, kid.get(b"AP").unwrap()).unwrap();
        let n = resolve_dict(&doc, ap.get(b"N").unwrap()).unwrap();
        assert!(n.has(b"Opt0"));
        assert!(n.has(b"Off"));
        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn delete_radio_kids_then_parent_gone() {
        let path = temp_pdf();
        let field = NewField::Radio {
            name: "grp".into(),
            required: false,
            options: vec![
                RadioOption {
                    value: "a".into(),
                    rect: InRect { x: 72.0, y: 100.0, w: 16.0, h: 16.0 },
                },
                RadioOption {
                    value: "b".into(),
                    rect: InRect { x: 72.0, y: 130.0, w: 16.0, h: 16.0 },
                },
            ],
        };
        create_field(&path, 0, &field).map_err(err_str).unwrap();
        delete_field(&path, 0, 1).map_err(err_str).unwrap();
        let doc = Document::load(&path).unwrap();
        assert_eq!(fields(&doc).len(), 1);
        assert_eq!(page_annots(&doc).len(), 1);
        drop(doc);
        delete_field(&path, 0, 0).map_err(err_str).unwrap();
        let doc = Document::load(&path).unwrap();
        assert_eq!(fields(&doc).len(), 0, "parent must go with its last kid");
        assert_eq!(page_annots(&doc).len(), 0);
        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn update_rect_regenerates_ap_bbox() {
        let path = temp_pdf();
        create_field(&path, 0, &text_field("mv")).map_err(err_str).unwrap();
        let upd = FieldUpdate {
            rect: Some(InRect { x: 10.0, y: 10.0, w: 300.0, h: 40.0 }),
            name: None,
            options: None,
            required: None,
        };
        update_field(&path, 0, 0, &upd).map_err(err_str).unwrap();
        let doc = Document::load(&path).unwrap();
        let widget = doc
            .get_dictionary(page_annots(&doc)[0].as_reference().unwrap())
            .unwrap();
        let ap = resolve_dict(&doc, widget.get(b"AP").unwrap()).unwrap();
        let n_id = ap.get(b"N").unwrap().as_reference().unwrap();
        let stream = doc.get_object(n_id).unwrap().as_stream().unwrap();
        let bbox = resolve_array(&doc, stream.dict.get(b"BBox").unwrap()).unwrap();
        let w = match bbox[2] {
            Object::Real(r) => r,
            Object::Integer(i) => i as f32,
            _ => panic!("BBox width not a number"),
        };
        assert!((w - 300.0).abs() < 0.01, "AP BBox width = {w}");
        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn tiny_rect_rejected() {
        let path = temp_pdf();
        let mut f = text_field("tiny");
        if let NewField::Text { rect, .. } = &mut f {
            rect.w = 4.0;
        }
        let err = create_field(&path, 0, &f).map_err(err_str).unwrap_err();
        assert!(err.contains("at least"), "unexpected: {err}");
        std::fs::remove_file(&path).ok();
    }
}
