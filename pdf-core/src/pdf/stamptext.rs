//! Text stamped onto pages in bulk: watermarks, and headers/footers with
//! automatic page numbers.
//!
//! Both write a small content stream per page and reference one embedded
//! subset font (see `embedfont`) from every page's `/Resources`. That is why
//! the text for all selected pages is resolved *before* the font is built —
//! "第 3 頁，共 12 頁" needs the digits of every page in the subset.
//!
//! Two placement rules matter:
//! - A watermark can go behind the page content (`behind`), which is what
//!   makes it readable under text rather than over it. Behind means our
//!   stream goes first in `/Contents`; on top means the existing streams get
//!   wrapped in `q`/`Q` first, so an unbalanced `q` in someone else's content
//!   cannot leak a transform into ours.
//! - Resource names are allocated fresh per call (`PdfEdF0`, `PdfEdF1`, …).
//!   Reusing a fixed name would repoint an *earlier* stamp's font at the new
//!   subset, and its glyph ids would render as different characters.

use std::path::Path;

use lopdf::{dictionary, Dictionary, Document, Object, ObjectId, Stream};
use serde::Deserialize;

use super::docutil::{media_box, page_ids, resolve_dict, save_atomic};
use super::embedfont;
use super::protect;

const MAX_FONT_SIZE: f32 = 400.0;
const MIN_FONT_SIZE: f32 = 1.0;

#[derive(Deserialize, Clone, Copy)]
pub struct Rgb {
    pub r: u8,
    pub g: u8,
    pub b: u8,
}

impl Rgb {
    fn to_fill(self) -> String {
        format!(
            "{:.3} {:.3} {:.3} rg",
            self.r as f32 / 255.0,
            self.g as f32 / 255.0,
            self.b as f32 / 255.0
        )
    }
}

// ---------------------------------------------------------------------------
// Watermark
// ---------------------------------------------------------------------------

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WatermarkOptions {
    pub text: String,
    /// 0-based pages; empty means every page.
    #[serde(default)]
    pub pages: Vec<u16>,
    #[serde(default = "default_watermark_size")]
    pub font_size: f32,
    /// 0.05–1.0. Fully transparent is not a useful outcome, only a confusing
    /// one, so the floor is above zero.
    #[serde(default = "default_opacity")]
    pub opacity: f32,
    #[serde(default = "default_grey")]
    pub color: Rgb,
    /// Counter-clockwise degrees; 45 is the conventional diagonal.
    #[serde(default = "default_rotation")]
    pub rotation: f32,
    /// Draw under the page content instead of over it.
    #[serde(default = "default_true")]
    pub behind: bool,
}

fn default_watermark_size() -> f32 {
    48.0
}
fn default_opacity() -> f32 {
    0.25
}
fn default_grey() -> Rgb {
    Rgb {
        r: 128,
        g: 128,
        b: 128,
    }
}
fn default_rotation() -> f32 {
    45.0
}
fn default_true() -> bool {
    true
}

pub fn watermark(path: &Path, opts: &WatermarkOptions) -> anyhow::Result<()> {
    let text = opts.text.trim();
    if text.is_empty() {
        anyhow::bail!("watermark text must not be empty");
    }
    check_font_size(opts.font_size, "watermark")?;
    if !(0.05..=1.0).contains(&opts.opacity) {
        anyhow::bail!("watermark opacity must be between 0.05 and 1.0");
    }
    if !opts.rotation.is_finite() {
        anyhow::bail!("watermark rotation must be finite");
    }

    protect::assert_editable(path)?;
    let mut doc = Document::load(path)?;
    let pages = page_ids(&doc);
    let targets = target_pages(&opts.pages, pages.len())?;

    let font = embedfont::embed(&mut doc, text)?;
    if font.is_empty() {
        anyhow::bail!("watermark text has no printable characters");
    }
    let hex = hex_string(&font.encode(text));
    let text_width = font.width(text, opts.font_size);
    let theta = opts.rotation.to_radians();
    let (sin, cos) = (theta.sin(), theta.cos());

    // One transparency state for the whole run; every page references it.
    let gs = doc.add_object(dictionary! {
        "Type" => "ExtGState",
        "CA" => Object::Real(opts.opacity),
        "ca" => Object::Real(opts.opacity),
    });

    for index in targets {
        let page = pages[index];
        let media = media_box(&doc, page)?;
        let font_name = add_resource(&mut doc, page, b"Font", Object::Reference(font.id))?;
        let gs_name = add_resource(&mut doc, page, b"ExtGState", Object::Reference(gs))?;

        // Place the text's own centre on the page centre, after rotation.
        let (cx, cy) = ((media[0] + media[2]) / 2.0, (media[1] + media[3]) / 2.0);
        let (hx, hy) = (text_width / 2.0, 0.35 * opts.font_size);
        let tx = cx - (cos * hx - sin * hy);
        let ty = cy - (sin * hx + cos * hy);

        let content = format!(
            "q\n/{gs_name} gs\n{fill}\nBT\n/{font_name} {size:.2} Tf\n\
             {cos:.5} {sin:.5} {neg_sin:.5} {cos:.5} {tx:.2} {ty:.2} Tm\n{hex} Tj\nET\nQ\n",
            fill = opts.color.to_fill(),
            size = opts.font_size,
            neg_sin = -sin,
        );
        splice_content(&mut doc, page, content.into_bytes(), opts.behind)?;
    }

    save_atomic(&mut doc, path)
}

// ---------------------------------------------------------------------------
// Header / footer
// ---------------------------------------------------------------------------

/// Six independent slots, matching Acrobat's header/footer grid. Every slot
/// runs through [`substitute`], so any of them can carry `{page}`.
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HeaderFooterOptions {
    #[serde(default)]
    pub header_left: String,
    #[serde(default)]
    pub header_center: String,
    #[serde(default)]
    pub header_right: String,
    #[serde(default)]
    pub footer_left: String,
    #[serde(default)]
    pub footer_center: String,
    #[serde(default)]
    pub footer_right: String,
    /// 0-based pages; empty means every page.
    #[serde(default)]
    pub pages: Vec<u16>,
    #[serde(default = "default_hf_size")]
    pub font_size: f32,
    #[serde(default = "default_black")]
    pub color: Rgb,
    /// Distance from the page edge to the text, in points.
    #[serde(default = "default_margin")]
    pub margin: f32,
    /// The number `{page}` shows on the document's first page. Lets a report
    /// whose cover is page 1 of another file start at 2, the way Acrobat's
    /// "start numbering at" does.
    #[serde(default = "default_start_number")]
    pub start_number: i64,
    /// What `{date}` expands to. Supplied by the caller rather than read from
    /// the clock here: the browser knows the user's timezone and date format,
    /// and the server does not.
    #[serde(default)]
    pub date: Option<String>,
}

fn default_hf_size() -> f32 {
    10.0
}
fn default_black() -> Rgb {
    Rgb { r: 0, g: 0, b: 0 }
}
fn default_margin() -> f32 {
    36.0
}
fn default_start_number() -> i64 {
    1
}

/// Hand-written rather than derived: a derived `Default` would give
/// `font_size: 0.0` and `start_number: 0`, which disagree with the values
/// the same struct gets when the fields are absent from JSON.
impl Default for HeaderFooterOptions {
    fn default() -> Self {
        Self {
            header_left: String::new(),
            header_center: String::new(),
            header_right: String::new(),
            footer_left: String::new(),
            footer_center: String::new(),
            footer_right: String::new(),
            pages: Vec::new(),
            font_size: default_hf_size(),
            color: default_black(),
            margin: default_margin(),
            start_number: default_start_number(),
            date: None,
        }
    }
}

/// Horizontal slot within a header or footer band.
#[derive(Clone, Copy)]
enum Align {
    Left,
    Center,
    Right,
}

/// One string to draw, after `{page}` and friends have been expanded.
struct Placed {
    text: String,
    /// Header band when true, footer band when false.
    header: bool,
    align: Align,
}

/// Everything one page needs drawn on it.
struct PageSlots {
    index: usize,
    placed: Vec<Placed>,
}

pub fn header_footer(path: &Path, opts: &HeaderFooterOptions) -> anyhow::Result<()> {
    let slots: [(&str, bool, Align); 6] = [
        (opts.header_left.trim(), true, Align::Left),
        (opts.header_center.trim(), true, Align::Center),
        (opts.header_right.trim(), true, Align::Right),
        (opts.footer_left.trim(), false, Align::Left),
        (opts.footer_center.trim(), false, Align::Center),
        (opts.footer_right.trim(), false, Align::Right),
    ];
    if slots.iter().all(|(text, _, _)| text.is_empty()) {
        anyhow::bail!("header/footer needs text in at least one position");
    }
    check_font_size(opts.font_size, "header/footer")?;
    if !opts.margin.is_finite() || opts.margin < 0.0 {
        anyhow::bail!("header/footer margin must not be negative");
    }

    protect::assert_editable(path)?;
    let mut doc = Document::load(path)?;
    let pages = page_ids(&doc);
    let page_count = pages.len();
    let targets = target_pages(&opts.pages, page_count)?;

    // Resolve every page's text first: the font subset has to cover the page
    // numbers that only exist after substitution.
    let resolved: Vec<PageSlots> = targets
        .iter()
        .map(|index| PageSlots {
            index: *index,
            placed: slots
                .iter()
                .filter(|(text, _, _)| !text.is_empty())
                .map(|(text, header, align)| Placed {
                    text: substitute(text, *index, page_count, opts),
                    header: *header,
                    align: *align,
                })
                .collect(),
        })
        .collect();

    let all_text: String = resolved
        .iter()
        .flat_map(|page| page.placed.iter().map(|slot| slot.text.as_str()))
        .collect();
    let font = embedfont::embed(&mut doc, &all_text)?;

    for PageSlots { index, placed } in resolved {
        let page = pages[index];
        let media = media_box(&doc, page)?;
        let (page_w, page_h) = (media[2] - media[0], media[3] - media[1]);
        if opts.margin * 2.0 + opts.font_size > page_h {
            anyhow::bail!("header/footer margin leaves no room on page {index}");
        }
        let font_name = add_resource(&mut doc, page, b"Font", Object::Reference(font.id))?;

        let mut content = format!("q\n{}\n", opts.color.to_fill());
        for slot in &placed {
            let width = font.width(&slot.text, opts.font_size);
            let x = match slot.align {
                Align::Left => media[0] + opts.margin,
                Align::Center => media[0] + (page_w - width) / 2.0,
                Align::Right => media[2] - opts.margin - width,
            }
            // A slot wider than the page would otherwise start off the left
            // edge and lose its first characters.
            .max(media[0]);
            let y = if slot.header {
                media[3] - opts.margin - opts.font_size
            } else {
                media[1] + opts.margin
            };
            content.push_str(&format!(
                "BT\n/{font_name} {size:.2} Tf\n{x:.2} {y:.2} Td\n{hex} Tj\nET\n",
                size = opts.font_size,
                hex = hex_string(&font.encode(&slot.text)),
            ));
        }
        content.push_str("Q\n");
        splice_content(&mut doc, page, content.into_bytes(), false)?;
    }

    save_atomic(&mut doc, path)
}

/// Expand `{page}`, `{pages}` and `{date}`. Unknown braces are left alone —
/// a footer that legitimately reads "{draft}" should survive.
fn substitute(text: &str, index: usize, page_count: usize, opts: &HeaderFooterOptions) -> String {
    let number = index as i64 + opts.start_number;
    text.replace("{page}", &number.to_string())
        .replace("{pages}", &page_count.to_string())
        .replace("{date}", opts.date.as_deref().unwrap_or(""))
}

// ---------------------------------------------------------------------------
// Page plumbing
// ---------------------------------------------------------------------------

fn check_font_size(size: f32, what: &str) -> anyhow::Result<()> {
    if !size.is_finite() || !(MIN_FONT_SIZE..=MAX_FONT_SIZE).contains(&size) {
        anyhow::bail!("{what} font size must be between {MIN_FONT_SIZE} and {MAX_FONT_SIZE}");
    }
    Ok(())
}

/// Selected page indices, sorted and de-duplicated. Empty means all pages.
fn target_pages(pages: &[u16], page_count: usize) -> anyhow::Result<Vec<usize>> {
    if page_count == 0 {
        anyhow::bail!("document has no pages");
    }
    if pages.is_empty() {
        return Ok((0..page_count).collect());
    }
    let mut out: Vec<usize> = Vec::with_capacity(pages.len());
    for page in pages {
        let index = *page as usize;
        if index >= page_count {
            anyhow::bail!("page {page} out of range");
        }
        out.push(index);
    }
    out.sort_unstable();
    out.dedup();
    Ok(out)
}

/// Put `value` in the page's `/Resources /<category>` under a name nothing
/// else uses, and return that name.
///
/// `/Resources` is normalized onto the page first: it is commonly inherited
/// from the page tree and shared by every page, and writing through that
/// shared dictionary would add the resource to pages the caller did not
/// select.
fn add_resource(
    doc: &mut Document,
    page_id: ObjectId,
    category: &[u8],
    value: Object,
) -> anyhow::Result<String> {
    let mut resources = inherited_resources(doc, page_id).unwrap_or_default();
    let mut group = resources
        .get(category)
        .ok()
        .and_then(|o| resolve_dict(doc, o))
        .unwrap_or_default();

    let name = free_name(&group);
    group.set(name.clone(), value);
    resources.set(category.to_vec(), group);
    doc.get_dictionary_mut(page_id)?.set("Resources", resources);
    Ok(name)
}

fn inherited_resources(doc: &Document, page_id: ObjectId) -> Option<Dictionary> {
    let mut current = Some(page_id);
    for _ in 0..64 {
        let id = current?;
        let dict = doc.get_dictionary(id).ok()?;
        if let Ok(res) = dict.get(b"Resources") {
            return resolve_dict(doc, res);
        }
        current = dict.get(b"Parent").ok().and_then(|p| p.as_reference().ok());
    }
    None
}

/// First `PdfEdN` not already present. Distinct from any name a producer is
/// likely to have written, and distinct from our own earlier stamps.
fn free_name(group: &Dictionary) -> String {
    (0..)
        .map(|n| format!("PdfEd{n}"))
        .find(|name| !group.has(name.as_bytes()))
        .expect("range is unbounded")
}

/// Add `content` to the page, either under everything already there or on
/// top of it.
fn splice_content(
    doc: &mut Document,
    page_id: ObjectId,
    content: Vec<u8>,
    behind: bool,
) -> anyhow::Result<()> {
    let ours = doc.add_object(Stream::new(Dictionary::new(), content));
    let existing = doc.get_dictionary(page_id)?.get(b"Contents").ok().cloned();

    let list = match existing {
        None => vec![Object::Reference(ours)],
        Some(contents) => {
            let existing = contents_to_list(doc, contents)?;
            if existing.is_empty() {
                vec![Object::Reference(ours)]
            } else if behind {
                let mut list = Vec::with_capacity(existing.len() + 1);
                list.push(Object::Reference(ours));
                list.extend(existing);
                list
            } else {
                // Balance whatever the page left on the graphics stack before
                // our stream runs.
                let open = doc.add_object(Stream::new(Dictionary::new(), b"q\n".to_vec()));
                let close = doc.add_object(Stream::new(Dictionary::new(), b"\nQ\n".to_vec()));
                let mut list = Vec::with_capacity(existing.len() + 3);
                list.push(Object::Reference(open));
                list.extend(existing);
                list.push(Object::Reference(close));
                list.push(Object::Reference(ours));
                list
            }
        }
    };
    doc.get_dictionary_mut(page_id)?
        .set("Contents", Object::Array(list));
    Ok(())
}

/// Resolve `/Contents` to a list of stream references, promoting an inline
/// stream so it can be referenced alongside ours.
fn contents_to_list(doc: &mut Document, obj: Object) -> anyhow::Result<Vec<Object>> {
    match obj {
        Object::Array(arr) => Ok(arr),
        Object::Stream(stream) => {
            let id = doc.add_object(Object::Stream(stream));
            Ok(vec![Object::Reference(id)])
        }
        Object::Reference(mut id) => {
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
        _ => anyhow::bail!("page /Contents is not a stream or array"),
    }
}

fn hex_string(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len() * 2 + 2);
    out.push('<');
    for byte in bytes {
        out.push_str(&format!("{byte:02X}"));
    }
    out.push('>');
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pdf::docutil::test_support::temp_pdf;

    /// Skips the body when the bundled font is absent (it is not in every
    /// checkout), so the suite stays green without silently passing.
    macro_rules! require_font {
        () => {
            if crate::pdf::font::full_font_bytes().is_none() {
                eprintln!("bundled font unavailable; skipping");
                return;
            }
        };
    }

    /// Concatenated content streams of one page, or `""` when the page has
    /// no `/Contents` at all — which is exactly what an unstamped page in a
    /// freshly built test document looks like.
    fn page_content(doc: &Document, index: usize) -> String {
        let page = page_ids(doc)[index];
        let Ok(contents) = doc.get_dictionary(page).unwrap().get(b"Contents") else {
            return String::new();
        };
        let list = match contents {
            Object::Array(arr) => arr.clone(),
            other => vec![other.clone()],
        };
        let mut out = String::new();
        for entry in list {
            if let Ok(id) = entry.as_reference() {
                if let Ok(stream) = doc.get_object(id).and_then(|o| o.as_stream()) {
                    let bytes = stream.decompressed_content().unwrap_or(stream.content.clone());
                    out.push_str(&String::from_utf8_lossy(&bytes));
                }
            }
        }
        out
    }

    fn wm(text: &str) -> WatermarkOptions {
        WatermarkOptions {
            text: text.into(),
            pages: Vec::new(),
            font_size: default_watermark_size(),
            opacity: default_opacity(),
            color: default_grey(),
            rotation: default_rotation(),
            behind: true,
        }
    }

    #[test]
    fn watermarks_every_page_by_default() {
        require_font!();
        let path = temp_pdf(3);
        watermark(&path, &wm("機密")).unwrap();

        let doc = Document::load(&path).unwrap();
        for index in 0..3 {
            let content = page_content(&doc, index);
            assert!(content.contains(" Tm"), "page {index}: {content}");
            assert!(content.contains(" gs"), "page {index} lost its opacity state");
        }
        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn watermarks_only_the_selected_pages() {
        require_font!();
        let path = temp_pdf(3);
        watermark(
            &path,
            &WatermarkOptions {
                pages: vec![2],
                ..wm("DRAFT")
            },
        )
        .unwrap();

        let doc = Document::load(&path).unwrap();
        assert!(page_content(&doc, 0).is_empty());
        assert!(page_content(&doc, 2).contains(" Tj"));
        std::fs::remove_file(&path).ok();
    }

    /// `behind` decides whether the stamp lands before or after the page's
    /// own streams — the difference between a readable watermark and one
    /// painted over the text.
    #[test]
    fn behind_puts_the_stamp_first_and_on_top_wraps_the_page() {
        require_font!();
        let path = temp_pdf(1);
        // Give the page some existing content to sit against.
        let mut doc = Document::load(&path).unwrap();
        let page = page_ids(&doc)[0];
        let body = doc.add_object(Stream::new(Dictionary::new(), b"1 0 0 RG\n".to_vec()));
        doc.get_dictionary_mut(page)
            .unwrap()
            .set("Contents", Object::Reference(body));
        doc.save(&path).unwrap();

        watermark(&path, &WatermarkOptions { behind: true, ..wm("A") }).unwrap();
        let doc = Document::load(&path).unwrap();
        let content = page_content(&doc, 0);
        assert!(
            content.find(" Tj").unwrap() < content.find("1 0 0 RG").unwrap(),
            "behind should draw before the page body: {content}"
        );

        let path2 = temp_pdf(1);
        let mut doc = Document::load(&path2).unwrap();
        let page = page_ids(&doc)[0];
        let body = doc.add_object(Stream::new(Dictionary::new(), b"1 0 0 RG\n".to_vec()));
        doc.get_dictionary_mut(page)
            .unwrap()
            .set("Contents", Object::Reference(body));
        doc.save(&path2).unwrap();

        watermark(&path2, &WatermarkOptions { behind: false, ..wm("A") }).unwrap();
        let doc = Document::load(&path2).unwrap();
        let content = page_content(&doc, 0);
        assert!(
            content.find(" Tj").unwrap() > content.find("1 0 0 RG").unwrap(),
            "on top should draw after the page body: {content}"
        );
        assert!(content.starts_with("q\n"), "page body should be wrapped: {content}");
        std::fs::remove_file(&path).ok();
        std::fs::remove_file(&path2).ok();
    }

    /// Two stamps must not share a resource name, or the first one's glyph
    /// ids get looked up in the second one's subset.
    #[test]
    fn a_second_stamp_allocates_a_new_resource_name() {
        require_font!();
        let path = temp_pdf(1);
        watermark(&path, &wm("AAA")).unwrap();
        watermark(&path, &wm("BBB")).unwrap();

        let doc = Document::load(&path).unwrap();
        let page = page_ids(&doc)[0];
        let resources = doc
            .get_dictionary(page)
            .unwrap()
            .get(b"Resources")
            .ok()
            .and_then(|o| resolve_dict(&doc, o))
            .unwrap();
        let fonts = resources
            .get(b"Font")
            .ok()
            .and_then(|o| resolve_dict(&doc, o))
            .unwrap();
        assert_eq!(fonts.len(), 2, "second stamp reused the first font slot");
        let content = page_content(&doc, 0);
        assert!(content.contains("/PdfEd0"), "{content}");
        assert!(content.contains("/PdfEd1"), "{content}");
        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn rejects_bad_watermark_options() {
        let path = temp_pdf(1);
        let err = watermark(&path, &wm("   ")).unwrap_err().to_string();
        assert!(err.contains("watermark text"), "{err}");

        let err = watermark(&path, &WatermarkOptions { opacity: 0.0, ..wm("x") })
            .unwrap_err()
            .to_string();
        assert!(err.contains("watermark opacity"), "{err}");

        let err = watermark(&path, &WatermarkOptions { font_size: 900.0, ..wm("x") })
            .unwrap_err()
            .to_string();
        assert!(err.contains("font size"), "{err}");

        let err = watermark(&path, &WatermarkOptions { pages: vec![7], ..wm("x") })
            .unwrap_err()
            .to_string();
        assert!(err.contains("out of range"), "{err}");
        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn substitutes_page_tokens_per_page() {
        let opts = HeaderFooterOptions {
            start_number: 1,
            ..Default::default()
        };
        assert_eq!(substitute("第 {page} 頁，共 {pages} 頁", 2, 10, &opts), "第 3 頁，共 10 頁");
        // An unknown token is content, not a typo to swallow.
        assert_eq!(substitute("{draft}", 0, 1, &opts), "{draft}");

        let offset = HeaderFooterOptions {
            start_number: 5,
            date: Some("2026-08-05".into()),
            ..Default::default()
        };
        assert_eq!(substitute("{page} / {date}", 0, 3, &offset), "5 / 2026-08-05");
    }

    #[test]
    fn draws_all_six_slots() {
        require_font!();
        let path = temp_pdf(2);
        header_footer(
            &path,
            &HeaderFooterOptions {
                header_left: "HL".into(),
                header_center: "HC".into(),
                header_right: "HR".into(),
                footer_left: "FL".into(),
                footer_center: "FC".into(),
                footer_right: "第 {page} 頁".into(),
                ..Default::default()
            },
        )
        .unwrap();

        let doc = Document::load(&path).unwrap();
        let content = page_content(&doc, 0);
        assert_eq!(content.matches(" Tj").count(), 6, "{content}");
        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn rejects_an_empty_header_footer() {
        let path = temp_pdf(1);
        let err = header_footer(&path, &HeaderFooterOptions::default())
            .unwrap_err()
            .to_string();
        assert!(err.contains("at least one position"), "{err}");
        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn rejects_a_margin_that_swallows_the_page() {
        require_font!();
        let path = temp_pdf(1);
        let err = header_footer(
            &path,
            &HeaderFooterOptions {
                footer_center: "x".into(),
                margin: 500.0,
                ..Default::default()
            },
        )
        .unwrap_err()
        .to_string();
        assert!(err.contains("no room"), "{err}");
        std::fs::remove_file(&path).ok();
    }
}
