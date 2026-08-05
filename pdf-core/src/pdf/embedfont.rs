//! Embed a subset of the bundled CJK font as a PDF composite font, for text
//! we draw into a content stream ourselves (`stamptext`).
//!
//! Different problem from `font.rs`, which subsets for PDFium's
//! `FPDFText_SetText` and therefore has to rebuild a unicode cmap so PDFium
//! can look glyphs up by character. Here we write the glyph ids into the
//! content stream directly, so the font goes in as CIDFontType2 with
//! `/Encoding /Identity-H` and `/CIDToGIDMap /Identity` — no cmap needed, and
//! the CID *is* the glyph id.
//!
//! We always embed rather than reaching for Helvetica: it is the only way
//! "機密" renders at all, and it means one code path with exact advance
//! widths, which centred and right-aligned text depends on.

use std::collections::{BTreeMap, BTreeSet};

use lopdf::{dictionary, Dictionary, Document, Object, ObjectId, Stream};
use subsetter::GlyphRemapper;

use super::font;

/// PDF glyph space: 1000 units to the em, whatever the font's own units are.
const PDF_EM: f32 = 1000.0;

pub(crate) struct EmbeddedFont {
    /// The `/Type0` font object to name in a page's `/Resources /Font`.
    pub(crate) id: ObjectId,
    char_to_gid: BTreeMap<char, u16>,
    /// Advance per new glyph id, already scaled to `PDF_EM`.
    advances: BTreeMap<u16, u16>,
}

impl EmbeddedFont {
    /// Glyph ids for `text` as the big-endian pairs an Identity-H string
    /// wants. Characters the font has no glyph for are dropped — they would
    /// render as nothing anyway, and `width` agrees with this.
    pub(crate) fn encode(&self, text: &str) -> Vec<u8> {
        let mut out = Vec::with_capacity(text.len() * 2);
        for ch in text.chars() {
            if let Some(gid) = self.char_to_gid.get(&ch) {
                out.extend_from_slice(&gid.to_be_bytes());
            }
        }
        out
    }

    /// Width of `text` at `font_size`, in points.
    pub(crate) fn width(&self, text: &str, font_size: f32) -> f32 {
        let units: f32 = text
            .chars()
            .filter_map(|ch| self.char_to_gid.get(&ch))
            .filter_map(|gid| self.advances.get(gid))
            .map(|w| *w as f32)
            .sum();
        units / PDF_EM * font_size
    }

    pub(crate) fn is_empty(&self) -> bool {
        self.char_to_gid.is_empty()
    }
}

/// Subset the bundled font down to `text`'s characters and add the font
/// objects to `doc`.
pub(crate) fn embed(doc: &mut Document, text: &str) -> anyhow::Result<EmbeddedFont> {
    let chars: BTreeSet<char> = text.chars().filter(|c| !c.is_control()).collect();
    if chars.is_empty() {
        anyhow::bail!("no printable characters to draw");
    }
    let full = font::full_font_bytes()
        .ok_or_else(|| anyhow::anyhow!("the bundled font is missing; cannot draw text"))?;
    let face = ttf_parser::Face::parse(full, 0)
        .map_err(|e| anyhow::anyhow!("font parse failed: {e}"))?;
    let upem = face.units_per_em() as f32;
    let to_pdf = |v: f32| (v / upem * PDF_EM).round();

    let mut remapper = GlyphRemapper::new();
    let mut char_to_gid = BTreeMap::new();
    let mut advances = BTreeMap::new();
    for ch in chars {
        let Some(gid) = face.glyph_index(ch) else {
            continue;
        };
        let new_gid = remapper.remap(gid.0);
        char_to_gid.insert(ch, new_gid);
        let advance = face.glyph_hor_advance(gid).unwrap_or(0) as f32;
        advances.insert(new_gid, to_pdf(advance) as u16);
    }
    if char_to_gid.is_empty() {
        anyhow::bail!("the bundled font has no glyphs for this text");
    }

    let subset = subsetter::subset(full, 0, &remapper)
        .map_err(|e| anyhow::anyhow!("font subset failed: {e}"))?;

    let mut file = Stream::new(
        dictionary! { "Length1" => subset.len() as i64 },
        subset,
    );
    // ~15 MB of source font subsets small, but a page of CJK still runs to
    // tens of kilobytes; a watermark should not visibly grow the document.
    let _ = file.compress();
    let file_id = doc.add_object(file);

    let bbox = face.global_bounding_box();
    let descriptor = doc.add_object(dictionary! {
        "Type" => "FontDescriptor",
        "FontName" => Object::Name(b"PdfEditorSubset".to_vec()),
        // Symbolic: the encoding is Identity, not one of the standard Latin
        // ones, so a viewer must not second-guess it against StandardEncoding.
        "Flags" => 4,
        "FontBBox" => vec![
            Object::Real(to_pdf(bbox.x_min as f32)),
            Object::Real(to_pdf(bbox.y_min as f32)),
            Object::Real(to_pdf(bbox.x_max as f32)),
            Object::Real(to_pdf(bbox.y_max as f32)),
        ],
        "ItalicAngle" => 0,
        "Ascent" => Object::Real(to_pdf(face.ascender() as f32)),
        "Descent" => Object::Real(to_pdf(face.descender() as f32)),
        "CapHeight" => Object::Real(to_pdf(face.capital_height().unwrap_or(face.ascender()) as f32)),
        // No StemV in a TrueType file; 80 is the conventional stand-in for a
        // regular weight and only affects substitution, which cannot happen
        // here because the font is embedded.
        "StemV" => 80,
        "FontFile2" => Object::Reference(file_id),
    });

    let cid_font = doc.add_object(dictionary! {
        "Type" => "Font",
        "Subtype" => "CIDFontType2",
        "BaseFont" => Object::Name(b"PdfEditorSubset".to_vec()),
        "CIDSystemInfo" => dictionary! {
            "Registry" => Object::string_literal("Adobe"),
            "Ordering" => Object::string_literal("Identity"),
            "Supplement" => 0,
        },
        "FontDescriptor" => Object::Reference(descriptor),
        "DW" => 1000,
        "W" => widths_array(&advances),
        "CIDToGIDMap" => Object::Name(b"Identity".to_vec()),
    });

    let to_unicode = doc.add_object(to_unicode_cmap(&char_to_gid));
    let id = doc.add_object(dictionary! {
        "Type" => "Font",
        "Subtype" => "Type0",
        "BaseFont" => Object::Name(b"PdfEditorSubset".to_vec()),
        "Encoding" => Object::Name(b"Identity-H".to_vec()),
        "DescendantFonts" => vec![Object::Reference(cid_font)],
        "ToUnicode" => Object::Reference(to_unicode),
    });

    Ok(EmbeddedFont {
        id,
        char_to_gid,
        advances,
    })
}

/// `/W` in the `c [w]` form — one entry per glyph, no run packing. The
/// subsets here are tens of glyphs, so the array stays small either way.
fn widths_array(advances: &BTreeMap<u16, u16>) -> Vec<Object> {
    let mut out = Vec::with_capacity(advances.len() * 2);
    for (gid, width) in advances {
        out.push(Object::Integer(*gid as i64));
        out.push(Object::Array(vec![Object::Integer(*width as i64)]));
    }
    out
}

/// A `/ToUnicode` CMap so the drawn text can still be selected, copied and
/// searched — page numbers and footers are content, not decoration.
fn to_unicode_cmap(char_to_gid: &BTreeMap<char, u16>) -> Stream {
    let mut body = String::from(
        "/CIDInit /ProcSet findresource begin\n\
         12 dict begin\n\
         begincmap\n\
         /CIDSystemInfo << /Registry (Adobe) /Ordering (UCS) /Supplement 0 >> def\n\
         /CMapName /Adobe-Identity-UCS def\n\
         /CMapType 2 def\n\
         1 begincodespacerange\n<0000> <FFFF>\nendcodespacerange\n",
    );
    // beginbfchar takes at most 100 entries per block (PDF 32000-1 §9.10.3).
    let entries: Vec<(u16, char)> = char_to_gid.iter().map(|(ch, gid)| (*gid, *ch)).collect();
    for chunk in entries.chunks(100) {
        body.push_str(&format!("{} beginbfchar\n", chunk.len()));
        for (gid, ch) in chunk {
            let mut buf = [0u16; 2];
            let mut utf16 = String::new();
            for unit in ch.encode_utf16(&mut buf).iter() {
                utf16.push_str(&format!("{unit:04X}"));
            }
            body.push_str(&format!("<{gid:04X}> <{utf16}>\n"));
        }
        body.push_str("endbfchar\n");
    }
    body.push_str("endcmap\nCMapName currentdict /CMap defineresource pop\nend\nend\n");

    let mut stream = Stream::new(Dictionary::new(), body.into_bytes());
    let _ = stream.compress();
    stream
}

#[cfg(test)]
mod tests {
    use super::*;

    fn doc() -> Document {
        Document::with_version("1.5")
    }

    #[test]
    fn encodes_two_bytes_per_glyph_and_measures_width() {
        let mut doc = doc();
        let Ok(font) = embed(&mut doc, "Page 1") else {
            eprintln!("bundled font unavailable; skipping");
            return;
        };
        assert_eq!(font.encode("Page 1").len(), 12);
        let width = font.width("Page 1", 10.0);
        assert!(width > 0.0 && width < 100.0, "width was {width}");
        // Advance widths must scale linearly with the point size.
        assert!((font.width("Page 1", 20.0) - width * 2.0).abs() < 0.01);
    }

    #[test]
    fn drops_characters_the_font_cannot_render() {
        let mut doc = doc();
        let Ok(font) = embed(&mut doc, "AB") else {
            eprintln!("bundled font unavailable; skipping");
            return;
        };
        // "Z" was never subsetted, so it contributes neither bytes nor width.
        assert_eq!(font.encode("ABZ"), font.encode("AB"));
        assert!((font.width("ABZ", 12.0) - font.width("AB", 12.0)).abs() < 0.01);
    }

    #[test]
    fn embeds_cjk_text() {
        let mut doc = doc();
        let Ok(font) = embed(&mut doc, "機密") else {
            eprintln!("bundled font unavailable; skipping");
            return;
        };
        assert_eq!(font.encode("機密").len(), 4);
        assert!(!font.is_empty());
    }

    #[test]
    fn rejects_text_with_nothing_to_draw() {
        let mut doc = doc();
        assert!(embed(&mut doc, "").is_err());
        assert!(embed(&mut doc, "\u{0}\u{1}").is_err());
    }
}
