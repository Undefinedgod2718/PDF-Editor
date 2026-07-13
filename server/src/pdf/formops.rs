//! AcroForm support: list form fields and fill them in.
//! Fields are addressed by (page, annotation index) — the index of the
//! widget annotation within the page's annotation collection.

use std::path::Path;

use pdfium_render::prelude::*;
use serde::{Deserialize, Serialize};

use super::with_document;

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FieldInfo {
    pub page: u16,
    /// Widget annotation index within the page's annotations.
    pub index: usize,
    pub name: String,
    pub field_type: String,
    /// Current text value (text / combo / list fields).
    pub value: Option<String>,
    /// Current checked state (checkbox / radio fields).
    pub checked: Option<bool>,
    /// Available options (combo / list fields).
    pub options: Option<Vec<String>>,
    /// Bounding rect in points, top-left origin.
    pub rect: Option<super::annots::OutRect>,
    /// Whether this backend can write the field (text/checkbox/radio).
    pub writable: bool,
}

#[derive(Deserialize)]
pub struct SetFieldBody {
    /// New value for text fields.
    pub value: Option<String>,
    /// New checked state for checkbox fields; radio buttons are selected
    /// by sending `checked: true` on the target widget.
    pub checked: Option<bool>,
}

fn collect_options(options: &PdfFormFieldOptions) -> Vec<String> {
    options
        .iter()
        .map(|o| o.label().cloned().unwrap_or_default())
        .collect()
}

fn field_info(
    page_index: u16,
    annot_index: usize,
    field: &PdfFormField,
    rect: Option<super::annots::OutRect>,
) -> FieldInfo {
    let (value, checked, options, writable) = match field {
        PdfFormField::Text(t) => (t.value(), None, None, true),
        PdfFormField::Checkbox(c) => (None, c.is_checked().ok(), None, true),
        PdfFormField::RadioButton(r) => (None, r.is_checked().ok(), None, true),
        PdfFormField::ComboBox(c) => (c.value(), None, Some(collect_options(c.options())), false),
        PdfFormField::ListBox(l) => (l.value(), None, Some(collect_options(l.options())), false),
        _ => (None, None, None, false),
    };
    FieldInfo {
        page: page_index,
        index: annot_index,
        name: field.name().unwrap_or_default(),
        field_type: format!("{:?}", field.field_type()),
        value,
        checked,
        options,
        rect,
        writable,
    }
}

/// `path` is still needed alongside the open document: button checked-state
/// is read from the raw file via lopdf (see below).
pub fn list_fields(doc: &PdfDocument, path: &Path) -> anyhow::Result<Vec<FieldInfo>> {
    // pdfium's is_checked() misreports unfilled radio groups as checked
    // (third-party verification finding), so button checked-state comes from
    // each widget's /AS name read via lopdf instead.
    let as_states = button_as_states(path).unwrap_or_default();
    let mut out = Vec::new();
    for (page_index, page) in doc.pages().iter().enumerate() {
        let page_height = page.height().value;
        for (annot_index, annot) in page.annotations().iter().enumerate() {
            if let PdfPageAnnotation::Widget(widget) = &annot {
                if let Some(field) = widget.form_field() {
                    let rect = annot.bounds().ok().map(|b| super::annots::OutRect {
                        x: b.left().value,
                        y: page_height - b.top().value,
                        w: b.right().value - b.left().value,
                        h: b.top().value - b.bottom().value,
                    });
                    let mut info = field_info(page_index as u16, annot_index, field, rect);
                    if matches!(
                        field,
                        PdfFormField::Checkbox(_) | PdfFormField::RadioButton(_)
                    ) {
                        info.checked =
                            Some(as_states.get(&(page_index as u16, annot_index)) == Some(&true));
                    }
                    out.push(info);
                }
            }
        }
    }
    Ok(out)
}

/// For every page annotation carrying an /AS name, record whether it is in
/// an "on" state (anything other than Off). Keyed by (page, annot index).
fn button_as_states(
    path: &Path,
) -> anyhow::Result<std::collections::HashMap<(u16, usize), bool>> {
    use lopdf::{Document, Object};
    let doc = Document::load(path)?;
    let mut out = std::collections::HashMap::new();
    for (page_no, page_id) in doc.get_pages() {
        let Ok(page) = doc.get_dictionary(page_id) else {
            continue;
        };
        let Some(annots) = page.get(b"Annots").ok().and_then(|a| resolve_array(&doc, a)) else {
            continue;
        };
        for (i, entry) in annots.iter().enumerate() {
            let dict = match entry {
                Object::Reference(id) => doc.get_dictionary(*id).ok().cloned(),
                Object::Dictionary(d) => Some(d.clone()),
                _ => None,
            };
            if let Some(dict) = dict {
                if let Ok(state) = dict.get(b"AS").and_then(|o| o.as_name()) {
                    out.insert((page_no as u16 - 1, i), state != b"Off");
                }
            }
        }
    }
    Ok(out)
}

fn resolve_array(doc: &lopdf::Document, obj: &lopdf::Object) -> Option<Vec<lopdf::Object>> {
    match obj {
        lopdf::Object::Array(a) => Some(a.clone()),
        lopdf::Object::Reference(id) => doc
            .get_object(*id)
            .ok()
            .and_then(|o| o.as_array().ok().cloned()),
        _ => None,
    }
}

pub fn set_field(
    pdfium: &Pdfium,
    path: &Path,
    page_index: u16,
    annot_index: usize,
    body: &SetFieldBody,
) -> anyhow::Result<()> {
    // Peek at the field type first; text goes through PDFium, checkbox and
    // radio go through lopdf (see set_button_lopdf for why).
    enum Kind {
        Text,
        Button,
    }
    let kind = {
        let doc = pdfium.load_pdf_from_file(path, None)?;
        let page = doc.pages().get(page_index)?;
        let annot = page
            .annotations()
            .get(annot_index as _)
            .map_err(|_| anyhow::anyhow!("annotation index {annot_index} out of range"))?;
        let PdfPageAnnotation::Widget(widget) = &annot else {
            anyhow::bail!("annotation {annot_index} is not a form field widget");
        };
        let Some(field) = widget.form_field() else {
            anyhow::bail!("widget {annot_index} has no form field");
        };
        match field {
            PdfFormField::Text(_) => Kind::Text,
            PdfFormField::Checkbox(_) | PdfFormField::RadioButton(_) => Kind::Button,
            other => anyhow::bail!(
                "field type {:?} is not writable in this version",
                other.field_type()
            ),
        }
    };

    match kind {
        Kind::Text => with_document(pdfium, path, |doc| {
            let page = doc.pages().get(page_index)?;
            let mut annot = page.annotations().get(annot_index as _)?;
            let PdfPageAnnotation::Widget(widget) = &mut annot else {
                anyhow::bail!("annotation {annot_index} is not a form field widget");
            };
            let Some(PdfFormField::Text(t)) = widget.form_field_mut() else {
                anyhow::bail!("not a text field");
            };
            let value = body
                .value
                .as_deref()
                .ok_or_else(|| anyhow::anyhow!("text field needs `value`"))?;
            t.set_value(value)?;
            Ok(())
        }),
        Kind::Button => {
            let checked = body
                .checked
                .ok_or_else(|| anyhow::anyhow!("checkbox/radio needs `checked`"))?;
            set_button_lopdf(path, page_index, annot_index, checked)
        }
    }
}

/// Set a checkbox / radio widget through lopdf dictionary surgery.
///
/// pdfium-render's `set_checked` writes `/AS` and `/V` as *string* objects
/// (`(/Yes)`), which violates the PDF spec (both must be *name* objects) and
/// leaves the tick invisible in conforming renderers. Here the widget's real
/// on-state name is read from its `/AP /N` dictionary, and `/AS` + `/V` are
/// written as proper names. For radio groups, `/V` is set on the parent field
/// and every sibling widget's `/AS` is updated to match.
fn set_button_lopdf(
    path: &Path,
    page_index: u16,
    annot_index: usize,
    checked: bool,
) -> anyhow::Result<()> {
    use lopdf::{Dictionary, Document, Object, ObjectId};

    let mut doc = Document::load(path)?;

    let page_id = *doc
        .get_pages()
        .get(&(page_index as u32 + 1))
        .ok_or_else(|| anyhow::anyhow!("page {page_index} out of range"))?;
    let annots = doc
        .get_dictionary(page_id)?
        .get(b"Annots")
        .ok()
        .and_then(|a| resolve_array(&doc, a))
        .ok_or_else(|| anyhow::anyhow!("page has no annotations"))?;
    let annot_id: ObjectId = annots
        .get(annot_index)
        .and_then(|o| o.as_reference().ok())
        .ok_or_else(|| anyhow::anyhow!("annotation index {annot_index} out of range"))?;

    // The on-state is whatever key in /AP /N isn't "Off" ("Yes" by convention
    // but arbitrary in real-world forms).
    fn on_state(doc: &Document, annot: &Dictionary) -> Vec<u8> {
        annot
            .get(b"AP")
            .ok()
            .and_then(|ap| resolve_dict(doc, ap))
            .and_then(|ap| ap.get(b"N").ok().and_then(|n| resolve_dict(doc, n)))
            .and_then(|n| {
                n.iter()
                    .map(|(k, _)| k.clone())
                    .find(|k| k.as_slice() != b"Off")
            })
            .unwrap_or_else(|| b"Yes".to_vec())
    }

    fn resolve_dict(doc: &Document, obj: &Object) -> Option<Dictionary> {
        match obj {
            Object::Dictionary(d) => Some(d.clone()),
            Object::Reference(id) => doc.get_dictionary(*id).ok().cloned(),
            _ => None,
        }
    }

    let annot_dict = doc.get_dictionary(annot_id)?.clone();
    let this_on = on_state(&doc, &annot_dict);
    let new_state = if checked { this_on.clone() } else { b"Off".to_vec() };

    // Radio widgets usually hang off a parent field holding /V and /Kids.
    let parent = annot_dict
        .get(b"Parent")
        .ok()
        .and_then(|p| p.as_reference().ok());

    let kids: Vec<ObjectId> = parent
        .and_then(|parent_id| {
            doc.get_dictionary(parent_id)
                .ok()
                .and_then(|p| p.get(b"Kids").ok().and_then(|k| resolve_array(&doc, k)))
        })
        .unwrap_or_default()
        .iter()
        .filter_map(|o| o.as_reference().ok())
        .collect();

    if let (Some(parent_id), false) = (parent, kids.is_empty()) {
        doc.get_dictionary_mut(parent_id)?
            .set("V", Object::Name(new_state.clone()));
        for kid_id in kids {
            let kid = doc.get_dictionary(kid_id)?.clone();
            let kid_on = on_state(&doc, &kid);
            let kid_state = if checked && kid_id == annot_id {
                kid_on
            } else {
                b"Off".to_vec()
            };
            doc.get_dictionary_mut(kid_id)?
                .set("AS", Object::Name(kid_state));
        }
    } else {
        // No parent, or a parent without /Kids (hierarchical checkbox):
        // write directly on this widget so the change is never silently lost.
        if let Some(parent_id) = parent {
            doc.get_dictionary_mut(parent_id)?
                .set("V", Object::Name(new_state.clone()));
        }
        let annot_mut = doc.get_dictionary_mut(annot_id)?;
        annot_mut.set("AS", Object::Name(new_state.clone()));
        annot_mut.set("V", Object::Name(new_state));
    }

    let mut bytes = Vec::new();
    doc.save_to(&mut bytes)?;
    let tmp = path.with_extension("pdf.tmp");
    std::fs::write(&tmp, &bytes)?;
    std::fs::rename(&tmp, path)?;
    Ok(())
}
