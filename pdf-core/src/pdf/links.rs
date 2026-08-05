//! Link annotations: internal jumps to another page and external URLs.
//!
//! lopdf dictionary surgery, like `formbuild` — PDFium cannot create link
//! annotations. Coordinates crossing the API are PDF points with a top-left
//! origin (project-wide convention, see annots.rs) and are flipped against
//! the page MediaBox here.
//!
//! External targets are restricted to http/https/mailto. A PDF `/URI` action
//! is a live link in every viewer, and the same slot accepts `javascript:`
//! and `file:` — neither of which a user drawing a rectangle on a page is
//! asking for, and both of which turn a shared document into an attack. The
//! allowlist is deliberate; widen it only with a reason.

use std::collections::HashMap;
use std::path::Path;

use lopdf::{dictionary, Document, Object, ObjectId};
use serde::{Deserialize, Serialize};

use super::annots::InRect;
use super::docutil::{
    media_box, page_id, page_ids, push_page_annot, resolve_array, resolve_dict, save_atomic, to_f32,
};
use super::outline::dest_page;
use super::protect;

/// Minimum clickable edge in points; smaller rects are always a mis-drag.
const MIN_SIZE: f32 = 4.0;
const ALLOWED_SCHEMES: &[&str] = &["http://", "https://", "mailto:"];

#[derive(Serialize)]
pub struct LinkInfo {
    /// Position in the page's `/Annots` array — the handle for deletion.
    pub index: usize,
    pub rect: OutRect,
    #[serde(flatten)]
    pub target: LinkTargetInfo,
}

#[derive(Serialize)]
pub struct OutRect {
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
}

#[derive(Serialize)]
#[serde(tag = "target", rename_all = "camelCase")]
pub enum LinkTargetInfo {
    /// Jump within this document.
    Page { page: u16 },
    /// External URL.
    Uri { url: String },
    /// A link we can see but not describe — a launch action, a remote GoTo,
    /// or a destination that no longer resolves. Listed so the user can
    /// delete it; never rewritten.
    Other,
}

#[derive(Deserialize)]
#[serde(tag = "target", rename_all = "camelCase")]
pub enum NewLinkTarget {
    Page { page: u16 },
    Uri { url: String },
}

#[derive(Deserialize)]
pub struct NewLink {
    pub rect: InRect,
    #[serde(flatten)]
    pub target: NewLinkTarget,
}

// ---------------------------------------------------------------------------
// Read
// ---------------------------------------------------------------------------

pub fn list(path: &Path, page_index: u16) -> anyhow::Result<Vec<LinkInfo>> {
    let doc = Document::load(path)?;
    let page = page_id(&doc, page_index)?;
    let media = media_box(&doc, page)?;
    let index_of: HashMap<ObjectId, u16> = page_ids(&doc)
        .into_iter()
        .enumerate()
        .map(|(i, id)| (id, i as u16))
        .collect();

    let annots = doc
        .get_dictionary(page)
        .ok()
        .and_then(|d| d.get(b"Annots").ok().and_then(|a| resolve_array(&doc, a)))
        .unwrap_or_default();

    let mut out = Vec::new();
    for (index, entry) in annots.iter().enumerate() {
        let Some(dict) = resolve_dict(&doc, entry) else {
            continue;
        };
        let is_link = dict
            .get(b"Subtype")
            .and_then(|o| o.as_name())
            .map(|n| n == b"Link")
            .unwrap_or(false);
        if !is_link {
            continue;
        }
        let Some(rect) = dict
            .get(b"Rect")
            .ok()
            .and_then(|o| resolve_array(&doc, o))
            .and_then(|arr| unflip_rect(&arr, &media))
        else {
            continue;
        };
        out.push(LinkInfo {
            index,
            rect,
            target: read_target(&doc, &dict, &index_of),
        });
    }
    Ok(out)
}

fn read_target(
    doc: &Document,
    dict: &lopdf::Dictionary,
    index_of: &HashMap<ObjectId, u16>,
) -> LinkTargetInfo {
    if let Ok(dest) = dict.get(b"Dest") {
        if let Some(page) = dest_page(doc, dest, index_of) {
            return LinkTargetInfo::Page { page };
        }
    }
    let Some(action) = dict.get(b"A").ok().and_then(|o| resolve_dict(doc, o)) else {
        return LinkTargetInfo::Other;
    };
    match action.get(b"S").and_then(|o| o.as_name()) {
        Ok(b"URI") => match action.get(b"URI") {
            // /URI is a byte string, not a text string: no UTF-16 case.
            Ok(Object::String(bytes, _)) => LinkTargetInfo::Uri {
                url: String::from_utf8_lossy(bytes).into_owned(),
            },
            _ => LinkTargetInfo::Other,
        },
        Ok(b"GoTo") => match action.get(b"D").ok().and_then(|d| dest_page(doc, d, index_of)) {
            Some(page) => LinkTargetInfo::Page { page },
            None => LinkTargetInfo::Other,
        },
        _ => LinkTargetInfo::Other,
    }
}

// ---------------------------------------------------------------------------
// Write
// ---------------------------------------------------------------------------

pub fn create(path: &Path, page_index: u16, link: &NewLink) -> anyhow::Result<()> {
    protect::assert_editable(path)?;
    let mut doc = Document::load(path)?;
    let page = page_id(&doc, page_index)?;
    let media = media_box(&doc, page)?;
    let rect = flip_rect(&link.rect, &media)?;

    let action_or_dest: (&str, Object) = match &link.target {
        NewLinkTarget::Page { page } => {
            let pages = page_ids(&doc);
            let target = *pages
                .get(*page as usize)
                .ok_or_else(|| anyhow::anyhow!("link target page {page} out of range"))?;
            (
                "Dest",
                Object::Array(vec![
                    Object::Reference(target),
                    "XYZ".into(),
                    Object::Null,
                    Object::Null,
                    Object::Null,
                ]),
            )
        }
        NewLinkTarget::Uri { url } => (
            "A",
            Object::Dictionary(dictionary! {
                "Type" => "Action",
                "S" => "URI",
                "URI" => Object::String(normalize_url(url)?.into_bytes(), lopdf::StringFormat::Literal),
            }),
        ),
    };

    let mut annot = dictionary! {
        "Type" => "Annot",
        "Subtype" => "Link",
        "Rect" => rect.iter().map(|v| Object::Real(*v)).collect::<Vec<_>>(),
        // No visible frame — Acrobat's default for a new link.
        "Border" => vec![0.into(), 0.into(), 0.into()],
        // /Print, so the link survives a print-to-PDF round trip.
        "F" => 4,
    };
    annot.set(action_or_dest.0, action_or_dest.1);

    let annot_id = doc.add_object(annot);
    push_page_annot(&mut doc, page, annot_id)?;
    save_atomic(&mut doc, path)
}

pub fn delete(path: &Path, page_index: u16, annot_index: usize) -> anyhow::Result<()> {
    protect::assert_editable(path)?;
    let mut doc = Document::load(path)?;
    let page = page_id(&doc, page_index)?;

    let mut annots = doc
        .get_dictionary(page)?
        .get(b"Annots")
        .ok()
        .and_then(|a| resolve_array(&doc, a))
        .ok_or_else(|| anyhow::anyhow!("page has no annotations"))?;
    let entry = annots
        .get(annot_index)
        .cloned()
        .ok_or_else(|| anyhow::anyhow!("annotation index {annot_index} out of range"))?;

    // Guard the index against a stale client: deleting whatever happens to sit
    // at that slot could take out a highlight the user still wants.
    let is_link = resolve_dict(&doc, &entry)
        .and_then(|d| d.get(b"Subtype").and_then(|o| o.as_name()).ok().map(|n| n == b"Link"))
        .unwrap_or(false);
    if !is_link {
        anyhow::bail!("annotation {annot_index} is not a link");
    }

    annots.remove(annot_index);
    doc.get_dictionary_mut(page)?.set("Annots", annots);
    if let Object::Reference(id) = entry {
        doc.objects.remove(&id);
    }
    save_atomic(&mut doc, path)
}

// ---------------------------------------------------------------------------
// URLs
// ---------------------------------------------------------------------------

/// Validate the scheme and percent-encode anything outside printable ASCII,
/// so a URL typed with Chinese characters survives as a byte string.
fn normalize_url(url: &str) -> anyhow::Result<String> {
    let trimmed = url.trim();
    if trimmed.is_empty() {
        anyhow::bail!("link url must not be empty");
    }
    let lower = trimmed.to_ascii_lowercase();
    if !ALLOWED_SCHEMES.iter().any(|s| lower.starts_with(s)) {
        anyhow::bail!("link url must start with http://, https:// or mailto:");
    }
    let mut out = String::with_capacity(trimmed.len());
    for byte in trimmed.bytes() {
        // Parens and backslash are the literal-string delimiters in a PDF
        // byte string; space and anything non-ASCII would confuse a viewer's
        // URL parser. `%` passes through untouched so a URL the user pasted
        // already percent-encoded ("…/a%20b") is not double-encoded into
        // "…/a%2520b".
        if byte.is_ascii_graphic() && !matches!(byte, b'(' | b')' | b'\\') {
            out.push(byte as char);
        } else {
            out.push_str(&format!("%{byte:02X}"));
        }
    }
    Ok(out)
}

// ---------------------------------------------------------------------------
// Geometry
// ---------------------------------------------------------------------------

/// Top-left-origin API rect → PDF `[x0, y0, x1, y1]` in MediaBox space.
fn flip_rect(rect: &InRect, media: &[f32; 4]) -> anyhow::Result<[f32; 4]> {
    let (page_w, page_h) = (media[2] - media[0], media[3] - media[1]);
    if !(rect.x.is_finite() && rect.y.is_finite() && rect.w.is_finite() && rect.h.is_finite()) {
        anyhow::bail!("link rect must be finite");
    }
    if rect.w < MIN_SIZE || rect.h < MIN_SIZE {
        anyhow::bail!("link rect must be at least {MIN_SIZE}pt on each side");
    }
    if rect.w > page_w || rect.h > page_h {
        anyhow::bail!("link rect is larger than the page");
    }
    let x = rect.x.clamp(0.0, page_w - rect.w);
    let y = rect.y.clamp(0.0, page_h - rect.h);
    let x0 = media[0] + x;
    let y0 = media[3] - y - rect.h;
    Ok([x0, y0, x0 + rect.w, y0 + rect.h])
}

/// PDF `/Rect` → top-left-origin API rect. Tolerates the corners being
/// stored in either order, which files in the wild do.
fn unflip_rect(arr: &[Object], media: &[f32; 4]) -> Option<OutRect> {
    let v: Vec<f32> = arr.iter().filter_map(to_f32).collect();
    if v.len() != 4 {
        return None;
    }
    let (x0, x1) = (v[0].min(v[2]), v[0].max(v[2]));
    let (y0, y1) = (v[1].min(v[3]), v[1].max(v[3]));
    Some(OutRect {
        x: x0 - media[0],
        y: media[3] - y1,
        w: x1 - x0,
        h: y1 - y0,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pdf::docutil::test_support::temp_pdf;

    fn rect() -> InRect {
        InRect {
            x: 100.0,
            y: 50.0,
            w: 200.0,
            h: 20.0,
        }
    }

    #[test]
    fn creates_and_lists_an_internal_link() {
        let path = temp_pdf(3);
        create(
            &path,
            0,
            &NewLink {
                rect: rect(),
                target: NewLinkTarget::Page { page: 2 },
            },
        )
        .unwrap();

        let links = list(&path, 0).unwrap();
        assert_eq!(links.len(), 1);
        assert!(matches!(links[0].target, LinkTargetInfo::Page { page: 2 }));
        // Round trip through the MediaBox flip lands back where it started.
        assert!((links[0].rect.x - 100.0).abs() < 0.01);
        assert!((links[0].rect.y - 50.0).abs() < 0.01);
        assert!((links[0].rect.w - 200.0).abs() < 0.01);
        assert!((links[0].rect.h - 20.0).abs() < 0.01);
        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn creates_and_lists_a_url_link() {
        let path = temp_pdf(1);
        create(
            &path,
            0,
            &NewLink {
                rect: rect(),
                target: NewLinkTarget::Uri {
                    url: "https://example.com/a b".into(),
                },
            },
        )
        .unwrap();

        let links = list(&path, 0).unwrap();
        assert_eq!(links.len(), 1);
        match &links[0].target {
            LinkTargetInfo::Uri { url } => assert_eq!(url, "https://example.com/a%20b"),
            other => panic!("expected a uri target, got {}", serde_json::to_string(other).unwrap()),
        }
        std::fs::remove_file(&path).ok();
    }

    /// The allowlist is the point of `normalize_url`; a regression here ships
    /// script execution to anyone who opens the document.
    #[test]
    fn rejects_script_and_file_urls() {
        for url in [
            "javascript:alert(1)",
            "JavaScript:alert(1)",
            "file:///C:/Windows/System32",
            "data:text/html,<script>alert(1)</script>",
            "  ",
        ] {
            let err = normalize_url(url).unwrap_err().to_string();
            assert!(err.contains("link url"), "{url} → {err}");
        }
    }

    #[test]
    fn percent_encodes_non_ascii_and_delimiters() {
        let url = normalize_url("https://example.com/報告(1).pdf").unwrap();
        assert!(url.starts_with("https://example.com/"));
        assert!(!url.contains('('), "{url}");
        assert!(url.is_ascii(), "{url}");
    }

    #[test]
    fn deletes_only_link_annotations() {
        let path = temp_pdf(1);
        create(
            &path,
            0,
            &NewLink {
                rect: rect(),
                target: NewLinkTarget::Uri {
                    url: "https://example.com".into(),
                },
            },
        )
        .unwrap();

        // Put a non-link annotation ahead of the link, then aim at slot 0.
        let mut doc = Document::load(&path).unwrap();
        let page = page_id(&doc, 0).unwrap();
        let note = doc.add_object(dictionary! {
            "Type" => "Annot",
            "Subtype" => "Text",
            "Rect" => vec![0.into(), 0.into(), 20.into(), 20.into()],
        });
        let mut annots = doc
            .get_dictionary(page)
            .unwrap()
            .get(b"Annots")
            .ok()
            .and_then(|a| resolve_array(&doc, a))
            .unwrap();
        annots.insert(0, Object::Reference(note));
        doc.get_dictionary_mut(page).unwrap().set("Annots", annots);
        doc.save(&path).unwrap();

        let err = delete(&path, 0, 0).unwrap_err().to_string();
        assert!(err.contains("is not a link"), "{err}");
        assert_eq!(list(&path, 0).unwrap().len(), 1);

        delete(&path, 0, 1).unwrap();
        assert!(list(&path, 0).unwrap().is_empty());
        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn rejects_a_rect_that_is_too_small_or_off_page() {
        let path = temp_pdf(1);
        let tiny = InRect { x: 10.0, y: 10.0, w: 2.0, h: 2.0 };
        let err = create(&path, 0, &NewLink { rect: tiny, target: NewLinkTarget::Page { page: 0 } })
            .unwrap_err()
            .to_string();
        assert!(err.contains("link rect"), "{err}");

        let huge = InRect { x: 0.0, y: 0.0, w: 5000.0, h: 20.0 };
        let err = create(&path, 0, &NewLink { rect: huge, target: NewLinkTarget::Page { page: 0 } })
            .unwrap_err()
            .to_string();
        assert!(err.contains("link rect"), "{err}");
        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn rejects_an_out_of_range_target_page() {
        let path = temp_pdf(2);
        let err = create(
            &path,
            0,
            &NewLink { rect: rect(), target: NewLinkTarget::Page { page: 9 } },
        )
        .unwrap_err()
        .to_string();
        assert!(err.contains("out of range"), "{err}");
        std::fs::remove_file(&path).ok();
    }
}
