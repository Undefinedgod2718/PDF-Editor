//! Export rendered pages as PNG/JPEG/TIFF/PPTX. All rendering happens on
//! the PDFium worker thread; this module is pure CPU work (render + encode)
//! given an already-open [`PdfDocument`].

use std::io::Cursor;

use image::codecs::jpeg::JpegEncoder;
use image::{DynamicImage, ExtendedColorType, ImageEncoder, ImageFormat};
use pdfium_render::prelude::*;
use zip::write::SimpleFileOptions;
use zip::{CompressionMethod, ZipWriter};

/// Raster / PPTX formats handled by this module (PDFium render + encode).
/// Office formats (docx/xlsx) stay at the API layer and never reach here.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExportFormat {
    Png,
    Jpg,
    Tiff,
    Pptx,
}

/// Result of an export: the encoded bytes plus enough metadata for the
/// HTTP layer to set `Content-Type` / `Content-Disposition`.
pub struct ExportOutput {
    pub bytes: Vec<u8>,
    pub content_type: &'static str,
    /// File extension (without dot) to append to the download filename.
    pub ext: &'static str,
}

/// Render `pages` (0-based indices, already validated against page count)
/// from `doc` at `scale` pixels-per-point and encode as `format`.
pub fn export(
    doc: &PdfDocument,
    format: ExportFormat,
    pages: &[u16],
    scale: f32,
    quality: u8,
) -> anyhow::Result<ExportOutput> {
    match format {
        ExportFormat::Png => export_raster(doc, pages, scale, RasterFormat::Png),
        ExportFormat::Jpg => export_raster(doc, pages, scale, RasterFormat::Jpg { quality }),
        ExportFormat::Tiff => export_tiff(doc, pages, scale),
        ExportFormat::Pptx => export_pptx(doc, pages, scale),
    }
}

/// Render one page to a [`DynamicImage`]. `scale` is pixels per PDF point
/// (1.0 = 72 dpi), matching the convention used by `ops::render_page`.
fn render_page_image(doc: &PdfDocument, index: u16, scale: f32) -> anyhow::Result<DynamicImage> {
    let page = doc.pages().get(index)?;
    let width = (page.width().value * scale).round().max(1.0) as i32;
    let config = PdfRenderConfig::new()
        .set_target_width(width)
        .render_form_data(true)
        .render_annotations(true);
    let bitmap = page.render_with_config(&config)?;
    Ok(bitmap.as_image())
}

fn encode_png(img: &DynamicImage) -> anyhow::Result<Vec<u8>> {
    let mut buf = Cursor::new(Vec::new());
    img.write_to(&mut buf, ImageFormat::Png)?;
    Ok(buf.into_inner())
}

/// PDFium bitmaps may carry an alpha channel; JPEG has none, so flatten to
/// RGB8 before encoding.
fn encode_jpeg(img: &DynamicImage, quality: u8) -> anyhow::Result<Vec<u8>> {
    let rgb = img.to_rgb8();
    let mut buf = Vec::new();
    let encoder = JpegEncoder::new_with_quality(&mut buf, quality);
    encoder.write_image(rgb.as_raw(), rgb.width(), rgb.height(), ExtendedColorType::Rgb8)?;
    Ok(buf)
}

#[derive(Clone, Copy)]
enum RasterFormat {
    Png,
    Jpg { quality: u8 },
}

/// Single page => raw image bytes. Multiple pages => a `Stored` (no
/// recompression needed, the image bytes are already compressed) zip of
/// `page-0001.png`/`.jpg` files, numbered by original page index + 1.
fn export_raster(
    doc: &PdfDocument,
    pages: &[u16],
    scale: f32,
    format: RasterFormat,
) -> anyhow::Result<ExportOutput> {
    let (content_type, ext): (&'static str, &'static str) = match format {
        RasterFormat::Png => ("image/png", "png"),
        RasterFormat::Jpg { .. } => ("image/jpeg", "jpg"),
    };

    if pages.len() == 1 {
        let img = render_page_image(doc, pages[0], scale)?;
        let bytes = match format {
            RasterFormat::Png => encode_png(&img)?,
            RasterFormat::Jpg { quality } => encode_jpeg(&img, quality)?,
        };
        return Ok(ExportOutput { bytes, content_type, ext });
    }

    let mut zip = ZipWriter::new(Cursor::new(Vec::new()));
    let options = SimpleFileOptions::default().compression_method(CompressionMethod::Stored);
    for &page in pages {
        let img = render_page_image(doc, page, scale)?;
        let bytes = match format {
            RasterFormat::Png => encode_png(&img)?,
            RasterFormat::Jpg { quality } => encode_jpeg(&img, quality)?,
        };
        let name = format!("page-{:04}.{}", page as u32 + 1, ext);
        zip.start_file(name, options)?;
        std::io::Write::write_all(&mut zip, &bytes)?;
    }
    let cursor = zip.finish()?;
    Ok(ExportOutput {
        bytes: cursor.into_inner(),
        content_type: "application/zip",
        ext: "zip",
    })
}

/// One multi-page TIFF (multiple IFDs), not a zip.
fn export_tiff(doc: &PdfDocument, pages: &[u16], scale: f32) -> anyhow::Result<ExportOutput> {
    let mut cursor = Cursor::new(Vec::new());
    {
        let mut encoder = tiff::encoder::TiffEncoder::new(&mut cursor)?;
        for &page in pages {
            let img = render_page_image(doc, page, scale)?;
            let rgb = img.to_rgb8();
            encoder.write_image::<tiff::encoder::colortype::RGB8>(
                rgb.width(),
                rgb.height(),
                rgb.as_raw(),
            )?;
        }
    }
    Ok(ExportOutput {
        bytes: cursor.into_inner(),
        content_type: "image/tiff",
        ext: "tiff",
    })
}

// ---------------------------------------------------------------------
// PPTX
// ---------------------------------------------------------------------

/// EMU (English Metric Units) per PDF point: 914400 EMU/inch / 72 pt/inch.
const EMU_PER_POINT: f64 = 914400.0 / 72.0;

fn pt_to_emu(pt: f32) -> i64 {
    (pt as f64 * EMU_PER_POINT).round() as i64
}

/// Render each page to PNG and assemble a minimal but valid .pptx (a
/// standard OOXML zip package) with no external pptx-writing crate.
fn export_pptx(doc: &PdfDocument, pages: &[u16], scale: f32) -> anyhow::Result<ExportOutput> {
    anyhow::ensure!(!pages.is_empty(), "no pages selected for export");

    // Slide size follows the first selected page's aspect ratio.
    let first_page = doc.pages().get(pages[0])?;
    let slide_cx = pt_to_emu(first_page.width().value).max(1);
    let slide_cy = pt_to_emu(first_page.height().value).max(1);

    struct Slide {
        png: Vec<u8>,
        off_x: i64,
        off_y: i64,
        ext_cx: i64,
        ext_cy: i64,
    }

    let mut slides = Vec::with_capacity(pages.len());
    for &page_index in pages {
        let page = doc.pages().get(page_index)?;
        let page_w = pt_to_emu(page.width().value).max(1);
        let page_h = pt_to_emu(page.height().value).max(1);

        // Fit the page into the slide box, preserving its own aspect ratio,
        // and center it (letterboxing when the aspect ratio differs from
        // the first page's).
        let slide_ar = slide_cx as f64 / slide_cy as f64;
        let page_ar = page_w as f64 / page_h as f64;
        let (ext_cx, ext_cy) = if page_ar > slide_ar {
            let cx = slide_cx;
            let cy = ((cx as f64) / page_ar).round() as i64;
            (cx, cy.max(1))
        } else {
            let cy = slide_cy;
            let cx = ((cy as f64) * page_ar).round() as i64;
            (cx.max(1), cy)
        };
        let off_x = (slide_cx - ext_cx) / 2;
        let off_y = (slide_cy - ext_cy) / 2;

        let img = render_page_image(doc, page_index, scale)?;
        let png = encode_png(&img)?;
        slides.push(Slide { png, off_x, off_y, ext_cx, ext_cy });
    }

    let mut zip = ZipWriter::new(Cursor::new(Vec::new()));
    let xml_opts = SimpleFileOptions::default().compression_method(CompressionMethod::Deflated);
    let png_opts = SimpleFileOptions::default().compression_method(CompressionMethod::Stored);

    let write_str = |zip: &mut ZipWriter<Cursor<Vec<u8>>>, name: &str, content: String| -> anyhow::Result<()> {
        zip.start_file(name, xml_opts)?;
        std::io::Write::write_all(zip, content.as_bytes())?;
        Ok(())
    };

    write_str(&mut zip, "[Content_Types].xml", content_types_xml(slides.len()))?;
    write_str(&mut zip, "_rels/.rels", root_rels_xml())?;
    write_str(&mut zip, "ppt/presentation.xml", presentation_xml(slides.len(), slide_cx, slide_cy))?;
    write_str(
        &mut zip,
        "ppt/_rels/presentation.xml.rels",
        presentation_rels_xml(slides.len()),
    )?;
    write_str(&mut zip, "ppt/slideMasters/slideMaster1.xml", slide_master_xml())?;
    write_str(
        &mut zip,
        "ppt/slideMasters/_rels/slideMaster1.xml.rels",
        slide_master_rels_xml(),
    )?;
    write_str(&mut zip, "ppt/slideLayouts/slideLayout1.xml", slide_layout_xml())?;
    write_str(
        &mut zip,
        "ppt/slideLayouts/_rels/slideLayout1.xml.rels",
        slide_layout_rels_xml(),
    )?;
    write_str(&mut zip, "ppt/theme/theme1.xml", theme_xml())?;

    for (i, slide) in slides.iter().enumerate() {
        let n = i + 1;
        write_str(
            &mut zip,
            &format!("ppt/slides/slide{n}.xml"),
            slide_xml(slide.off_x, slide.off_y, slide.ext_cx, slide.ext_cy),
        )?;
        write_str(
            &mut zip,
            &format!("ppt/slides/_rels/slide{n}.xml.rels"),
            slide_rels_xml(n),
        )?;
        zip.start_file(format!("ppt/media/image{n}.png"), png_opts)?;
        std::io::Write::write_all(&mut zip, &slide.png)?;
    }

    let cursor = zip.finish()?;
    Ok(ExportOutput {
        bytes: cursor.into_inner(),
        content_type: "application/vnd.openxmlformats-officedocument.presentationml.presentation",
        ext: "pptx",
    })
}

fn content_types_xml(slide_count: usize) -> String {
    let mut overrides = String::new();
    overrides.push_str(r#"<Override PartName="/ppt/presentation.xml" ContentType="application/vnd.openxmlformats-officedocument.presentationml.presentation.main+xml"/>"#);
    overrides.push_str(r#"<Override PartName="/ppt/slideMasters/slideMaster1.xml" ContentType="application/vnd.openxmlformats-officedocument.presentationml.slideMaster+xml"/>"#);
    overrides.push_str(r#"<Override PartName="/ppt/slideLayouts/slideLayout1.xml" ContentType="application/vnd.openxmlformats-officedocument.presentationml.slideLayout+xml"/>"#);
    overrides.push_str(r#"<Override PartName="/ppt/theme/theme1.xml" ContentType="application/vnd.openxmlformats-officedocument.theme+xml"/>"#);
    for n in 1..=slide_count {
        overrides.push_str(&format!(
            r#"<Override PartName="/ppt/slides/slide{n}.xml" ContentType="application/vnd.openxmlformats-officedocument.presentationml.slide+xml"/>"#
        ));
    }
    format!(
        r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types">
<Default Extension="rels" ContentType="application/vnd.openxmlformats-package.relationships+xml"/>
<Default Extension="xml" ContentType="application/xml"/>
<Default Extension="png" ContentType="image/png"/>
{overrides}
</Types>"#
    )
}

fn root_rels_xml() -> String {
    r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships">
<Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/officeDocument" Target="ppt/presentation.xml"/>
</Relationships>"#
        .to_string()
}

fn presentation_xml(slide_count: usize, cx: i64, cy: i64) -> String {
    let mut sld_ids = String::new();
    for i in 0..slide_count {
        let id = 256 + i;
        let r_id = 2 + i; // rId1 = slideMaster1, rId2.. = slides
        sld_ids.push_str(&format!(r#"<p:sldId id="{id}" r:id="rId{r_id}"/>"#));
    }
    format!(
        r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<p:presentation xmlns:a="http://schemas.openxmlformats.org/drawingml/2006/main" xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships" xmlns:p="http://schemas.openxmlformats.org/presentationml/2006/main">
<p:sldMasterIdLst><p:sldMasterId id="2147483648" r:id="rId1"/></p:sldMasterIdLst>
<p:sldIdLst>{sld_ids}</p:sldIdLst>
<p:sldSz cx="{cx}" cy="{cy}"/>
<p:notesSz cx="6858000" cy="9144000"/>
</p:presentation>"#
    )
}

fn presentation_rels_xml(slide_count: usize) -> String {
    let mut rels = String::new();
    rels.push_str(r#"<Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/slideMaster" Target="slideMasters/slideMaster1.xml"/>"#);
    for i in 0..slide_count {
        let n = i + 1;
        let r_id = 2 + i;
        rels.push_str(&format!(
            r#"<Relationship Id="rId{r_id}" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/slide" Target="slides/slide{n}.xml"/>"#
        ));
    }
    format!(
        r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships">
{rels}
</Relationships>"#
    )
}

fn slide_master_xml() -> String {
    r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<p:sldMaster xmlns:a="http://schemas.openxmlformats.org/drawingml/2006/main" xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships" xmlns:p="http://schemas.openxmlformats.org/presentationml/2006/main">
<p:cSld>
<p:spTree>
<p:nvGrpSpPr><p:cNvPr id="1" name=""/><p:cNvGrpSpPr/><p:nvPr/></p:nvGrpSpPr>
<p:grpSpPr/>
</p:spTree>
</p:cSld>
<p:clrMap bg1="lt1" tx1="dk1" bg2="lt2" tx2="dk2" accent1="accent1" accent2="accent2" accent3="accent3" accent4="accent4" accent5="accent5" accent6="accent6" hlink="hlink" folHlink="folHlink"/>
<p:sldLayoutIdLst><p:sldLayoutId id="2147483649" r:id="rId1"/></p:sldLayoutIdLst>
</p:sldMaster>"#
        .to_string()
}

fn slide_master_rels_xml() -> String {
    r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships">
<Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/slideLayout" Target="../slideLayouts/slideLayout1.xml"/>
<Relationship Id="rId2" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/theme" Target="../theme/theme1.xml"/>
</Relationships>"#
        .to_string()
}

fn slide_layout_xml() -> String {
    r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<p:sldLayout xmlns:a="http://schemas.openxmlformats.org/drawingml/2006/main" xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships" xmlns:p="http://schemas.openxmlformats.org/presentationml/2006/main" type="blank" preserve="1">
<p:cSld name="Blank">
<p:spTree>
<p:nvGrpSpPr><p:cNvPr id="1" name=""/><p:cNvGrpSpPr/><p:nvPr/></p:nvGrpSpPr>
<p:grpSpPr/>
</p:spTree>
</p:cSld>
<p:clrMapOvr><a:overrideClrMapping bg1="lt1" tx1="dk1" bg2="lt2" tx2="dk2" accent1="accent1" accent2="accent2" accent3="accent3" accent4="accent4" accent5="accent5" accent6="accent6" hlink="hlink" folHlink="folHlink"/></p:clrMapOvr>
</p:sldLayout>"#
        .to_string()
}

fn slide_layout_rels_xml() -> String {
    r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships">
<Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/slideMaster" Target="../slideMasters/slideMaster1.xml"/>
</Relationships>"#
        .to_string()
}

fn theme_xml() -> String {
    r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<a:theme xmlns:a="http://schemas.openxmlformats.org/drawingml/2006/main" name="Office Theme">
<a:themeElements>
<a:clrScheme name="Office">
<a:dk1><a:sysClr val="windowText" lastClr="000000"/></a:dk1>
<a:lt1><a:sysClr val="window" lastClr="FFFFFF"/></a:lt1>
<a:dk2><a:srgbClr val="44546A"/></a:dk2>
<a:lt2><a:srgbClr val="E7E6E6"/></a:lt2>
<a:accent1><a:srgbClr val="4472C4"/></a:accent1>
<a:accent2><a:srgbClr val="ED7D31"/></a:accent2>
<a:accent3><a:srgbClr val="A5A5A5"/></a:accent3>
<a:accent4><a:srgbClr val="FFC000"/></a:accent4>
<a:accent5><a:srgbClr val="5B9BD5"/></a:accent5>
<a:accent6><a:srgbClr val="70AD47"/></a:accent6>
<a:hlink><a:srgbClr val="0563C1"/></a:hlink>
<a:folHlink><a:srgbClr val="954F72"/></a:folHlink>
</a:clrScheme>
<a:fontScheme name="Office">
<a:majorFont><a:latin typeface="Calibri Light"/><a:ea typeface=""/><a:cs typeface=""/></a:majorFont>
<a:minorFont><a:latin typeface="Calibri"/><a:ea typeface=""/><a:cs typeface=""/></a:minorFont>
</a:fontScheme>
<a:fmtScheme name="Office">
<a:fillStyleLst>
<a:solidFill><a:schemeClr val="phClr"/></a:solidFill>
<a:solidFill><a:schemeClr val="phClr"/></a:solidFill>
<a:solidFill><a:schemeClr val="phClr"/></a:solidFill>
</a:fillStyleLst>
<a:lnStyleLst>
<a:ln w="6350"><a:solidFill><a:schemeClr val="phClr"/></a:solidFill></a:ln>
<a:ln w="12700"><a:solidFill><a:schemeClr val="phClr"/></a:solidFill></a:ln>
<a:ln w="19050"><a:solidFill><a:schemeClr val="phClr"/></a:solidFill></a:ln>
</a:lnStyleLst>
<a:effectStyleLst>
<a:effectStyle><a:effectLst/></a:effectStyle>
<a:effectStyle><a:effectLst/></a:effectStyle>
<a:effectStyle><a:effectLst/></a:effectStyle>
</a:effectStyleLst>
<a:bgFillStyleLst>
<a:solidFill><a:schemeClr val="phClr"/></a:solidFill>
<a:solidFill><a:schemeClr val="phClr"/></a:solidFill>
<a:solidFill><a:schemeClr val="phClr"/></a:solidFill>
</a:bgFillStyleLst>
</a:fmtScheme>
</a:themeElements>
</a:theme>"#
        .to_string()
}

fn slide_xml(off_x: i64, off_y: i64, ext_cx: i64, ext_cy: i64) -> String {
    format!(
        r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<p:sld xmlns:a="http://schemas.openxmlformats.org/drawingml/2006/main" xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships" xmlns:p="http://schemas.openxmlformats.org/presentationml/2006/main">
<p:cSld>
<p:spTree>
<p:nvGrpSpPr><p:cNvPr id="1" name=""/><p:cNvGrpSpPr/><p:nvPr/></p:nvGrpSpPr>
<p:grpSpPr/>
<p:pic>
<p:nvPicPr>
<p:cNvPr id="2" name="Picture 1"/>
<p:cNvPicPr><a:picLocks noChangeAspect="1"/></p:cNvPicPr>
<p:nvPr/>
</p:nvPicPr>
<p:blipFill>
<a:blip r:embed="rId1"/>
<a:stretch><a:fillRect/></a:stretch>
</p:blipFill>
<p:spPr>
<a:xfrm><a:off x="{off_x}" y="{off_y}"/><a:ext cx="{ext_cx}" cy="{ext_cy}"/></a:xfrm>
<a:prstGeom prst="rect"><a:avLst/></a:prstGeom>
</p:spPr>
</p:pic>
</p:spTree>
</p:cSld>
<p:clrMapOvr><a:overrideClrMapping bg1="lt1" tx1="dk1" bg2="lt2" tx2="dk2" accent1="accent1" accent2="accent2" accent3="accent3" accent4="accent4" accent5="accent5" accent6="accent6" hlink="hlink" folHlink="folHlink"/></p:clrMapOvr>
</p:sld>"#
    )
}

fn slide_rels_xml(n: usize) -> String {
    format!(
        r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships">
<Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/image" Target="../media/image{n}.png"/>
</Relationships>"#
    )
}
