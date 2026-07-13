//! Per-annotation font subsetting. `subsetter` strips the cmap table
//! (PDF CID fonts don't need it), but PDFium's FPDFText_SetText resolves
//! unicode through the font's cmap — so a minimal cmap (format 4, BMP)
//! is rebuilt here and spliced back into the subset font.

use std::collections::BTreeMap;
use std::sync::OnceLock;

use subsetter::GlyphRemapper;

/// Full CJK font bytes, read once (the TTF is ~15 MB).
pub fn full_font_bytes() -> Option<&'static [u8]> {
    static FONT: OnceLock<Option<Vec<u8>>> = OnceLock::new();
    FONT.get_or_init(|| {
        let path = std::env::var("PDF_EDITOR_FONT")
            .unwrap_or_else(|_| "fonts/GenSenRoundedTW-R.ttf".into());
        match std::fs::read(&path) {
            Ok(bytes) => Some(bytes),
            Err(e) => {
                tracing::warn!(
                    "CJK font not loaded ({path}): {e}; freeText falls back to Helvetica"
                );
                None
            }
        }
    })
    .as_deref()
}

/// Subset the full font down to the glyphs used by `text`, returning a
/// standalone TTF with a rebuilt unicode cmap. Characters outside the BMP
/// or missing from the font are silently dropped (they render as nothing).
pub fn subset_for_text(full: &[u8], text: &str) -> anyhow::Result<Vec<u8>> {
    let face = ttf_parser::Face::parse(full, 0)
        .map_err(|e| anyhow::anyhow!("font parse failed: {e}"))?;

    let mut remapper = GlyphRemapper::new();
    let mut char_to_new_gid: BTreeMap<u16, u16> = BTreeMap::new();
    for ch in text.chars() {
        let code = ch as u32;
        if code > 0xFFFF || code == 0xFFFF {
            continue; // BMP-only cmap format 4
        }
        if char_to_new_gid.contains_key(&(code as u16)) {
            continue;
        }
        if let Some(gid) = face.glyph_index(ch) {
            let new_gid = remapper.remap(gid.0);
            char_to_new_gid.insert(code as u16, new_gid);
        }
    }
    if char_to_new_gid.is_empty() {
        anyhow::bail!("no glyphs found in font for the given text");
    }

    let subset = subsetter::subset(full, 0, &remapper)
        .map_err(|e| anyhow::anyhow!("font subset failed: {e}"))?;
    inject_cmap(&subset, &char_to_new_gid)
}

/// Build a cmap table (format 4, platform 3 / encoding 1) with one segment
/// per character, then append it to the sfnt table directory.
fn build_cmap(mapping: &BTreeMap<u16, u16>) -> Vec<u8> {
    let seg_count = (mapping.len() + 1) as u16; // + final 0xFFFF segment
    let seg_count_x2 = seg_count * 2;
    let search_range = 2 * (2u16.pow((seg_count as f32).log2().floor() as u32));
    let entry_selector = (search_range as f32 / 2.0).log2() as u16;
    let range_shift = seg_count_x2.saturating_sub(search_range);

    let mut sub = Vec::new();
    let push16 = |v: &mut Vec<u8>, x: u16| v.extend_from_slice(&x.to_be_bytes());

    push16(&mut sub, 4); // format
    let length = 16 + 8 * seg_count as usize; // fixed header + 4 arrays (no glyphIdArray)
    push16(&mut sub, length as u16);
    push16(&mut sub, 0); // language
    push16(&mut sub, seg_count_x2);
    push16(&mut sub, search_range);
    push16(&mut sub, entry_selector);
    push16(&mut sub, range_shift);
    // endCode[]
    for &c in mapping.keys() {
        push16(&mut sub, c);
    }
    push16(&mut sub, 0xFFFF);
    push16(&mut sub, 0); // reservedPad
    // startCode[]
    for &c in mapping.keys() {
        push16(&mut sub, c);
    }
    push16(&mut sub, 0xFFFF);
    // idDelta[]: gid = (char + delta) mod 65536, one segment per char
    for (&c, &gid) in mapping {
        push16(&mut sub, gid.wrapping_sub(c));
    }
    push16(&mut sub, 1); // 0xFFFF + 1 = 0 (.notdef)
    // idRangeOffset[]
    for _ in 0..seg_count {
        push16(&mut sub, 0);
    }

    let mut cmap = Vec::new();
    push16(&mut cmap, 0); // version
    push16(&mut cmap, 1); // numTables
    push16(&mut cmap, 3); // platform: Windows
    push16(&mut cmap, 1); // encoding: Unicode BMP
    cmap.extend_from_slice(&12u32.to_be_bytes()); // subtable offset
    cmap.extend_from_slice(&sub);
    cmap
}

/// Append `cmap` to an sfnt font that lacks one, rewriting the table
/// directory. Table checksums are left as-is / zero — FreeType (and thus
/// PDFium) does not verify them.
fn inject_cmap(font: &[u8], mapping: &BTreeMap<u16, u16>) -> anyhow::Result<Vec<u8>> {
    if font.len() < 12 {
        anyhow::bail!("font too small");
    }
    let num_tables = u16::from_be_bytes([font[4], font[5]]) as usize;
    let dir_end = 12 + num_tables * 16;
    if font.len() < dir_end {
        anyhow::bail!("truncated table directory");
    }

    let new_count = (num_tables + 1) as u16;
    let entry_selector = (new_count as f32).log2().floor() as u16;
    let search_range = 16 * 2u16.pow(entry_selector as u32);
    let range_shift = new_count * 16 - search_range;

    // All existing table payloads shift by one directory entry (16 bytes),
    // padded to 4-byte alignment.
    let shift = 16usize;
    let mut out = Vec::with_capacity(font.len() + shift + 4096);
    out.extend_from_slice(&font[0..4]); // sfnt version
    out.extend_from_slice(&new_count.to_be_bytes());
    out.extend_from_slice(&search_range.to_be_bytes());
    out.extend_from_slice(&entry_selector.to_be_bytes());
    out.extend_from_slice(&range_shift.to_be_bytes());

    // Existing directory entries with shifted offsets, kept sorted by tag —
    // "cmap" sorts before most tags, so collect, add, sort, emit.
    struct Entry {
        tag: [u8; 4],
        checksum: u32,
        offset: u32,
        length: u32,
    }
    let mut entries = Vec::with_capacity(num_tables + 1);
    for i in 0..num_tables {
        let e = &font[12 + i * 16..12 + i * 16 + 16];
        entries.push(Entry {
            tag: [e[0], e[1], e[2], e[3]],
            checksum: u32::from_be_bytes([e[4], e[5], e[6], e[7]]),
            offset: u32::from_be_bytes([e[8], e[9], e[10], e[11]]) + shift as u32,
            length: u32::from_be_bytes([e[12], e[13], e[14], e[15]]),
        });
    }
    let cmap = build_cmap(mapping);
    let cmap_offset = {
        // cmap payload goes at the (4-aligned) end of the shifted font body
        let body_end = font.len() + shift;
        ((body_end + 3) / 4 * 4) as u32
    };
    entries.push(Entry {
        tag: *b"cmap",
        checksum: 0,
        offset: cmap_offset,
        length: cmap.len() as u32,
    });
    entries.sort_by_key(|e| e.tag);

    for e in &entries {
        out.extend_from_slice(&e.tag);
        out.extend_from_slice(&e.checksum.to_be_bytes());
        out.extend_from_slice(&e.offset.to_be_bytes());
        out.extend_from_slice(&e.length.to_be_bytes());
    }
    // body: everything after the original directory, at its original relative
    // position (offsets were shifted by exactly `shift`)
    out.extend_from_slice(&font[dir_end..]);
    while out.len() % 4 != 0 {
        out.push(0);
    }
    debug_assert_eq!(out.len(), cmap_offset as usize);
    out.extend_from_slice(&cmap);
    Ok(out)
}
