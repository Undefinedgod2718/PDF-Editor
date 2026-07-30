//! OCR text-layer injection: recognizes scanned page raster images with
//! Tesseract and embeds the words as an invisible text layer, positioned by
//! the word's own transform (`translate`/`scale`) so font metrics never
//! matter — the layer is never rendered, only searched and copied.
//!
//! Pages that already carry extractable text are skipped (OCR would only
//! duplicate/garble an existing layer), unless `OcrOptions::force`.

use std::path::{Path, PathBuf};

use image::RgbaImage;
use pdfium_render::prelude::*;
use tesseract_rs::TesseractAPI;

use super::{font, ops, protect};

/// A recognized word, in pixel coordinates of the image that was OCR'd
/// (origin top-left, matching `image`/PDFium bitmap convention).
#[derive(Debug, Clone)]
pub struct OcrWord {
    pub text: String,
    pub left: i32,
    pub top: i32,
    pub right: i32,
    pub bottom: i32,
    /// 0-100 (Tesseract's own confidence scale).
    pub confidence: f32,
}

/// Resolve the tessdata directory: `PDF_EDITOR_TESSDATA` env var, else
/// `<exe_dir>/tessdata`, else `./tessdata` (dev, cwd = server/). Mirrors
/// `font::full_font_bytes` / `sidecar::resolve_python`'s resolution order.
fn tessdata_dir() -> anyhow::Result<PathBuf> {
    if let Ok(p) = std::env::var("PDF_EDITOR_TESSDATA") {
        let p = PathBuf::from(p);
        anyhow::ensure!(p.is_dir(), "PDF_EDITOR_TESSDATA={} is not a directory", p.display());
        return Ok(p);
    }
    let mut candidates: Vec<PathBuf> = Vec::new();
    if let Some(exe_dir) = std::env::current_exe().ok().and_then(|p| p.parent().map(|d| d.to_path_buf())) {
        candidates.push(exe_dir.join("tessdata"));
    }
    candidates.push(PathBuf::from("tessdata"));
    candidates
        .into_iter()
        .find(|p| p.is_dir())
        .ok_or_else(|| {
            anyhow::anyhow!(
                "tessdata directory not found; set PDF_EDITOR_TESSDATA or install \
                 eng.traineddata/chi_tra.traineddata next to the exe"
            )
        })
}

/// Known Tesseract language codes → human labels for the language picker.
/// Anything else found in the tessdata dir still shows, with its raw code
/// as the label — that's what lets a new `.traineddata` dropped in by an
/// operator appear without a code change here.
fn language_label(code: &str) -> Option<&'static str> {
    match code {
        "eng" => Some("英文"),
        "chi_tra" => Some("繁體中文"),
        _ => None,
    }
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct OcrLanguage {
    pub code: String,
    pub label: String,
}

/// List installed Tesseract language models (`*.traineddata` in the
/// resolved tessdata dir) for the OCR dialog's language picker. `osd`
/// (orientation/script detection data, not a recognizable language) is
/// excluded. Sorted by code for a stable order.
pub fn available_languages() -> anyhow::Result<Vec<OcrLanguage>> {
    let dir = tessdata_dir()?;
    let mut out = Vec::new();
    for entry in std::fs::read_dir(&dir)? {
        let path = entry?.path();
        if path.extension().and_then(|e| e.to_str()) != Some("traineddata") {
            continue;
        }
        let Some(code) = path.file_stem().and_then(|s| s.to_str()) else {
            continue;
        };
        if code == "osd" {
            continue;
        }
        let label = language_label(code).map(str::to_string).unwrap_or_else(|| code.to_string());
        out.push(OcrLanguage { code: code.to_string(), label });
    }
    out.sort_by(|a, b| a.code.cmp(&b.code));
    Ok(out)
}

/// Initialize a Tesseract engine for `langs`. Callers should create this once
/// per document and reuse it across `recognize_words` calls — `init` reloads
/// and parses the traineddata models from disk, which is expensive enough
/// that redoing it per page would dominate OCR time on multi-page documents.
/// `psm` is Tesseract's page-segmentation mode (`tessedit_pageseg_mode`),
/// never set before this and therefore left on the library's own default.
///
/// **That default is 6 (single uniform block), not 3.** The `tesseract` CLI
/// defaults to 3 and it is easy to assume the library does too; it does not.
/// Measured on a Traditional Chinese scan with known ground truth, by
/// character error rate: 3 → 44%, 4 → 44%, 6 → 49%, 11 → 42% accurate.
/// Defaulting this to 3 "because that is Tesseract's default" would have cost
/// five points of accuracy on every document.
///
/// It is exposed because layout analysis is per-document: a mode that reads
/// this page best may drop regions on a two-column one.
pub fn init_engine(langs: &str, psm: u8) -> anyhow::Result<TesseractAPI> {
    let datapath = tessdata_dir()?;
    let api = TesseractAPI::new();
    api.init(&datapath, langs)
        .map_err(|e| anyhow::anyhow!("tesseract init ({langs} @ {}) failed: {e}", datapath.display()))?;
    if let Some(mode) = page_seg_mode(psm) {
        api.set_page_seg_mode(mode)
            .map_err(|e| anyhow::anyhow!("tesseract set_page_seg_mode({psm}) failed: {e}"))?;
    }
    Ok(api)
}

/// 只放行「整頁辨識」有意義的幾種。單字/單行/OSD-only 那些是給呼叫端已經
/// 切好版面時用的，對整頁掃描不適用，收到就當作沒指定、沿用預設。
fn page_seg_mode(psm: u8) -> Option<tesseract_rs::TessPageSegMode> {
    use tesseract_rs::TessPageSegMode::*;
    match psm {
        3 => Some(PSM_AUTO),
        4 => Some(PSM_SINGLE_COLUMN),
        6 => Some(PSM_SINGLE_BLOCK),
        11 => Some(PSM_SPARSE_TEXT),
        _ => None,
    }
}

/// Recognize words in `image` at word granularity using an already-`init`'d
/// engine (see `init_engine`). Words with empty/whitespace-only text are
/// dropped. Returns `(words, truncated)` — `truncated` is true only when
/// `next_word` failed mid-page (remaining words unread). `get_current_word`
/// Err is ignored: Tesseract routinely yields that for non-text blocks and
/// empty pages, so counting it as "lost words" would lie.
pub fn recognize_words(
    api: &TesseractAPI,
    image: &RgbaImage,
) -> anyhow::Result<(Vec<OcrWord>, bool)> {
    // Leptonica's SetImage handles 1/3/4 bytes-per-pixel; RGB8 keeps this
    // simple and avoids any question over alpha handling.
    let rgb = image::DynamicImage::ImageRgba8(image.clone()).to_rgb8();
    let (width, height) = rgb.dimensions();
    let bytes_per_pixel = 3;
    let bytes_per_line = width as i32 * bytes_per_pixel;
    api.set_image(rgb.as_raw(), width as i32, height as i32, bytes_per_pixel, bytes_per_line)
        .map_err(|e| anyhow::anyhow!("tesseract set_image failed: {e}"))?;
    api.recognize()
        .map_err(|e| anyhow::anyhow!("tesseract recognize failed: {e}"))?;

    let iter = api
        .get_iterator()
        .map_err(|e| anyhow::anyhow!("tesseract get_iterator failed: {e}"))?;

    let mut words = Vec::new();
    let mut truncated = false;
    loop {
        // Non-text blocks / empty regions often return Err here — skip, don't
        // invent a "lost word" counter from Tesseract's normal noise.
        if let Ok((text, left, top, right, bottom, confidence)) = iter.get_current_word() {
            if !text.trim().is_empty() {
                words.push(OcrWord { text, left, top, right, bottom, confidence });
            }
        }
        match iter.next_word() {
            Ok(true) => continue,
            Ok(false) => break,
            Err(e) => {
                tracing::warn!("ocr: iterator advance failed, stopping page early: {e}");
                truncated = true;
                break;
            }
        }
    }
    Ok((words, truncated))
}

#[derive(Debug, Clone)]
pub struct OcrOptions {
    /// Render/recognition DPI. 300 is Tesseract's usual sweet spot.
    pub dpi: f32,
    /// Tesseract language spec, e.g. `"eng+chi_tra"`.
    pub langs: String,
    /// Drop recognized words below this confidence (0-100).
    pub min_confidence: f32,
    /// OCR pages even if they already have extractable text.
    pub force: bool,
    /// Tesseract page-segmentation mode. 6 (single block) is the library's
    /// own default and measured best (49% vs 44%) — see `init_engine`.
    pub psm: u8,
    /// 送進 Tesseract 前的影像處理，見 `Preprocess`。
    pub preprocess: Preprocess,
}

impl Default for OcrOptions {
    fn default() -> Self {
        Self {
            dpi: 300.0,
            langs: "eng+chi_tra".to_string(),
            min_confidence: 60.0,
            force: false,
            psm: 6,
            preprocess: Preprocess::None,
        }
    }
}

#[derive(Debug, Default, serde::Serialize)]
pub struct OcrStats {
    pub pages_processed: usize,
    pub pages_skipped_existing_text: usize,
    pub words_added: usize,
    pub words_skipped_low_confidence: usize,
    /// Non-ASCII words dropped because no CJK font was available to encode
    /// them (Standard14 Helvetica can't carry Traditional Chinese).
    pub words_skipped_no_font: usize,
    /// Pages where the word iterator failed mid-advance — remaining words
    /// on that page were not read. A boolean page count, not a fake word tally.
    pub pages_truncated: usize,
}

/// A page already counts as "has text" above this many characters — a
/// stray page number or watermark shouldn't block OCR of the scan under it.
const EXISTING_TEXT_THRESHOLD: usize = 10;

/// Shared page-progress counter for a running OCR job. Updated with plain
/// atomics from the PDFium worker thread (synchronous, no async involved);
/// read from the HTTP polling handler on the tokio side. No channel needed
/// — this is just "how far along is the loop", not a stream of events.
#[derive(Debug, Default)]
pub struct OcrProgress {
    pages_done: std::sync::atomic::AtomicUsize,
    pages_total: std::sync::atomic::AtomicUsize,
}

impl OcrProgress {
    pub fn set_total(&self, total: usize) {
        self.pages_total.store(total, std::sync::atomic::Ordering::Relaxed);
    }

    fn inc_done(&self) {
        self.pages_done.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    }

    pub fn snapshot(&self) -> (usize, usize) {
        (
            self.pages_done.load(std::sync::atomic::Ordering::Relaxed),
            self.pages_total.load(std::sync::atomic::Ordering::Relaxed),
        )
    }
}

/// OCR every page of the document at `path` that lacks extractable text
/// (or every page, with `opts.force`), embedding recognized words as an
/// invisible text layer. Returns the new document's bytes — callers save
/// this as a new document, matching `compress::compress`'s shape; the
/// original file on disk is untouched. `progress` is updated once per page
/// (processed or skipped) so a caller polling from another thread can show
/// "page X of Y" instead of a single opaque spinner.
pub fn ocr_document(
    pdfium: &Pdfium,
    path: &Path,
    opts: &OcrOptions,
    progress: &OcrProgress,
) -> anyhow::Result<(Vec<u8>, OcrStats)> {
    protect::assert_editable(path)?;
    let bytes = std::fs::read(path)?;
    let mut doc = pdfium.load_pdf_from_byte_vec(bytes, None)?;

    let mut stats = OcrStats::default();
    let full_font = font::full_font_bytes();
    let page_count = doc.pages().len();
    progress.set_total(page_count as usize);

    // Lazy: skip all-text PDFs without paying tessdata load. Once created,
    // reuse across pages — re-Init per page would dominate multi-page OCR.
    let mut engine: Option<TesseractAPI> = None;

    for index in 0..page_count {
        if !opts.force {
            let existing = ops::page_text(&doc, index)?;
            if existing.text.trim().chars().count() > EXISTING_TEXT_THRESHOLD {
                stats.pages_skipped_existing_text += 1;
                progress.inc_done();
                continue;
            }
        }

        if engine.is_none() {
            engine = Some(init_engine(&opts.langs, opts.psm)?);
        }
        let rendered = ops::render_page_image(&doc, index, opts.dpi / 72.0)?;
        let image = preprocess_image(&rendered, opts.preprocess);
        let (all_words, truncated) = recognize_words(engine.as_ref().unwrap(), &image)?;
        if truncated {
            stats.pages_truncated += 1;
        }
        let words: Vec<OcrWord> = all_words
            .into_iter()
            .filter(|w| {
                let keep = w.confidence >= opts.min_confidence;
                if !keep {
                    stats.words_skipped_low_confidence += 1;
                }
                keep
            })
            .collect();
        if words.is_empty() {
            stats.pages_processed += 1;
            progress.inc_done();
            continue;
        }

        // Font token must be taken before the page borrows the document
        // (same constraint as annots::create_on_doc / textedit::insert_line).
        let joined: String = words.iter().map(|w| w.text.as_str()).collect::<Vec<_>>().join(" ");
        let cjk_font = full_font
            .and_then(|full| {
                font::subset_for_text(full, &joined)
                    .map_err(|e| tracing::warn!("ocr font subset failed: {e}"))
                    .ok()
            })
            .and_then(|subset| {
                doc.fonts_mut()
                    .load_true_type_from_bytes(&subset, true)
                    .map_err(|e| tracing::warn!("ocr subset font load failed: {e:?}"))
                    .ok()
            });
        let fallback_font = doc.fonts_mut().helvetica();

        let mut page = doc.pages().get(index)?;
        let page_height = page.height().value;
        let px_to_pt = 72.0 / opts.dpi;

        for word in &words {
            let is_ascii = word.text.is_ascii();
            // `PdfFontToken` (from `helvetica()` / `load_true_type_from_bytes`)
            // is a `Copy` handle, not an owned font — safe to reuse per word.
            let font_for_word = match (cjk_font, is_ascii) {
                (Some(f), _) => f,
                (None, true) => fallback_font,
                (None, false) => {
                    stats.words_skipped_no_font += 1;
                    continue;
                }
            };

            let left_pt = word.left as f32 * px_to_pt;
            let bottom_pt = word.bottom as f32 * px_to_pt;
            let height_pt = ((word.bottom - word.top) as f32 * px_to_pt).max(1.0);
            let width_pt = (word.right - word.left) as f32 * px_to_pt;
            let baseline_y = page_height - bottom_pt;

            let mut text_obj =
                PdfPageTextObject::new(&doc, &word.text, font_for_word, PdfPoints::new(height_pt))?;
            text_obj.set_render_mode(PdfPageTextRenderMode::Invisible)?;

            // Scale to the OCR-measured width — the embedded font's own
            // glyph metrics are approximate at best for scanned text, so
            // matching width matters more than matching font shape (it's
            // invisible; shape is never seen).
            if let Ok(natural_width) = text_obj.width() {
                if natural_width.value > 0.01 {
                    let scale_x = (width_pt / natural_width.value).clamp(0.05, 20.0);
                    text_obj.scale(scale_x, 1.0)?;
                }
            }
            text_obj.translate(PdfPoints::new(left_pt), PdfPoints::new(baseline_y))?;

            page.objects_mut().add_text_object(text_obj)?;
            stats.words_added += 1;
        }
        page.regenerate_content()?;
        stats.pages_processed += 1;
        progress.inc_done();
    }

    let out = doc.save_to_bytes()?;
    Ok((out, stats))
}

#[cfg(test)]
mod tests {
    use super::*;
    use pdfium_render::prelude::Pdfium;

    /// Binds pdfium the same way `engine.rs`'s worker does, trying the repo's
    /// `server/` dir (cargo test cwd = crate root, so `../server`) before
    /// falling back to the system library.
    fn bind_pdfium() -> Pdfium {
        let candidates = ["../server", "./"];
        for dir in candidates {
            if let Ok(b) = Pdfium::bind_to_library(Pdfium::pdfium_platform_library_name_at_path(dir)) {
                return Pdfium::new(b);
            }
        }
        Pdfium::new(Pdfium::bind_to_system_library().expect("no pdfium binding found"))
    }

    /// Render `deploy/acceptance/fixtures/chinese.pdf` page 0 at 300dpi and
    /// confirm Tesseract (eng+chi_tra) recovers *some* plausible words.
    ///
    /// Ignored by default: the fixture (`deploy/acceptance/gen_fixtures.py`
    /// output) and tessdata (`PDF_EDITOR_TESSDATA` / `server/tessdata`) are
    /// not checked in, so a plain `cargo test` on a fresh checkout would
    /// otherwise fail here. Run explicitly with `cargo test -- --ignored`
    /// after generating fixtures and installing traineddata.
    #[test]
    #[ignore = "requires generated fixtures + tessdata, see doc comment"]
    fn recognizes_words_from_fixture_page() {
        let pdfium = bind_pdfium();
        let bytes = std::fs::read("../deploy/acceptance/fixtures/chinese.pdf")
            .expect("fixture PDF missing");
        let doc = pdfium.load_pdf_from_byte_vec(bytes, None).expect("load fixture");
        let image = super::super::ops::render_page_image(&doc, 0, 300.0 / 72.0)
            .expect("render page 0");

        let engine = init_engine("eng+chi_tra", 6).expect("init_engine");
        let (words, truncated) = recognize_words(&engine, &image).expect("recognize_words");
        assert!(!truncated, "fixture page should not truncate mid-iterator");
        assert!(!words.is_empty(), "expected at least one recognized word");
        let avg_conf: f32 = words.iter().map(|w| w.confidence).sum::<f32>() / words.len() as f32;
        println!(
            "recognized {} words, avg confidence {avg_conf:.1}: {:?}",
            words.len(),
            words.iter().take(5).map(|w| &w.text).collect::<Vec<_>>()
        );
        assert!(avg_conf > 30.0, "average confidence implausibly low: {avg_conf}");
    }

    /// End-to-end: `ocr_document` on the fixture (forced, since it already
    /// has a real text layer that would otherwise be skipped) should embed
    /// an invisible word layer and produce a larger, still-loadable PDF.
    ///
    /// Ignored by default — see `recognizes_words_from_fixture_page`.
    #[test]
    #[ignore = "requires generated fixtures + tessdata, see doc comment"]
    fn ocr_document_embeds_invisible_words() {
        let pdfium = bind_pdfium();
        let path = Path::new("../deploy/acceptance/fixtures/chinese.pdf");
        let before_len = std::fs::metadata(path).unwrap().len();

        let opts = OcrOptions { force: true, min_confidence: 0.0, ..OcrOptions::default() };
        let progress = OcrProgress::default();
        let (out, stats) = ocr_document(&pdfium, path, &opts, &progress).expect("ocr_document");

        println!("stats: {stats:?}");
        assert_eq!(stats.pages_processed, 1);
        assert!(stats.words_added > 0, "expected at least one invisible word embedded");
        assert!(out.len() as u64 >= before_len, "output should not shrink after adding a text layer");

        // Round-trip: the output must still be a valid, loadable PDF.
        let reloaded = pdfium.load_pdf_from_byte_vec(out, None).expect("reload OCR'd output");
        assert_eq!(reloaded.pages().len(), 1);
    }
}

// ---------- 前處理實驗（見 preprocess 的說明）----------

/// 送進 Tesseract 之前對頁面影像做的處理。
///
/// Tesseract 內部本來就會做 Otsu 二值化，所以「再幫它二值化一次」不必然有幫助。
///
/// **實測結論：四種都沒有增益。** 對一份 300 dpi 光柵化的繁中頁（有 ground
/// truth，用字元錯誤率 CER 衡量）：原樣 CER 0.511、otsu 0.506、gray 0.518、
/// contrast 0.518、sharpen 0.517。otsu 那 0.005 在雜訊範圍內——它修對了「億」
/// 和「溫哥華」，卻在別處弄壞等量的字。
///
/// 上面那份是**合成掃描**，所以後來又拿一張真實的手機翻拍發票（小字、窄欄、
/// 含 QR code）重測，用 12 個關鍵字串命中率評分：none 10/12、otsu 9/12、
/// sharpen 9/12、**contrast 3/12**。結論一致：none 最好。
///
/// `contrast` 在兩份樣本上都更差，真實那張尤其糟——百分位拉伸對「本來對比就
/// 夠好」的文件是破壞性的，41 個字因此掉到信心門檻以下。留著它是因為它存在的
/// 理由（低對比、泛黃的舊掃描）兩份樣本都沒有涵蓋，但**選它之前先量**。
///
/// 預設維持 None：沒有量到好處的東西不該偷偷生效。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Preprocess {
    /// 原樣送出（PDFium 給的抗鋸齒彩圖）。
    None,
    /// 只轉灰階。
    Gray,
    /// 灰階 + 全域 Otsu 二值化。
    Otsu,
    /// 灰階 + 百分位對比拉伸（把 2%/98% 分位拉到 0/255）。
    Contrast,
    /// 灰階 + unsharp mask 銳化。
    Sharpen,
}

impl Preprocess {
    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "none" => Some(Self::None),
            "gray" => Some(Self::Gray),
            "otsu" => Some(Self::Otsu),
            "contrast" => Some(Self::Contrast),
            "sharpen" => Some(Self::Sharpen),
            _ => None,
        }
    }
}

/// 取灰階值（BT.601 luma；跟 image crate 的 grayscale 一致）。
fn luma(px: &image::Rgba<u8>) -> u8 {
    let [r, g, b, _] = px.0;
    ((0.299 * r as f32) + (0.587 * g as f32) + (0.114 * b as f32)).round() as u8
}

/// Otsu 門檻：最大化類間變異數。標準做法，直方圖一次掃完。
fn otsu_threshold(hist: &[u32; 256], total: u32) -> u8 {
    let sum: f64 = hist.iter().enumerate().map(|(i, n)| i as f64 * *n as f64).sum();
    let (mut sum_b, mut w_b, mut best_var, mut best_t) = (0.0f64, 0u32, -1.0f64, 0u8);
    for t in 0..256 {
        w_b += hist[t];
        if w_b == 0 {
            continue;
        }
        let w_f = total - w_b;
        if w_f == 0 {
            break;
        }
        sum_b += t as f64 * hist[t] as f64;
        let m_b = sum_b / w_b as f64;
        let m_f = (sum - sum_b) / w_f as f64;
        let var = w_b as f64 * w_f as f64 * (m_b - m_f) * (m_b - m_f);
        if var > best_var {
            best_var = var;
            best_t = t as u8;
        }
    }
    best_t
}

/// 依 `mode` 產生要送進 Tesseract 的影像。回傳仍是 RgbaImage，因為
/// `recognize_words` 的介面吃這個型別——灰階/二值結果寫成 R=G=B。
///
/// 量測這些模式時踩過一個坑，記在這裡：用 `difflib.SequenceMatcher` 比對中文
/// 時**必須關掉 autojunk**。序列超過 200 個元素時它會把出現頻率 >1% 的元素當
/// 雜訊忽略，中文常用字幾乎全中，比值會變得毫無意義——gray/contrast/sharpen
/// 一度看起來只有 7% 相似度，實際上它們的準確率跟原樣一樣是 48%。
pub fn preprocess_image(img: &RgbaImage, mode: Preprocess) -> RgbaImage {
    if mode == Preprocess::None {
        return img.clone();
    }
    let (w, h) = img.dimensions();
    let mut gray: Vec<u8> = img.pixels().map(luma).collect();

    match mode {
        Preprocess::None | Preprocess::Gray => {}
        Preprocess::Otsu => {
            let mut hist = [0u32; 256];
            for &g in &gray {
                hist[g as usize] += 1;
            }
            let t = otsu_threshold(&hist, (w * h) as u32);
            for g in gray.iter_mut() {
                *g = if *g > t { 255 } else { 0 };
            }
        }
        Preprocess::Contrast => {
            // 百分位裁切比 min/max 穩：單一雜訊點不會把整條曲線壓平。
            let mut hist = [0u32; 256];
            for &g in &gray {
                hist[g as usize] += 1;
            }
            let total = (w * h) as u32;
            let pick = |frac: f32| -> u8 {
                let target = (total as f32 * frac) as u32;
                let mut acc = 0u32;
                for (i, n) in hist.iter().enumerate() {
                    acc += n;
                    if acc >= target {
                        return i as u8;
                    }
                }
                255
            };
            let (lo, hi) = (pick(0.02), pick(0.98));
            if hi > lo {
                let span = (hi - lo) as f32;
                for g in gray.iter_mut() {
                    let v = ((*g as f32 - lo as f32) / span * 255.0).clamp(0.0, 255.0);
                    *g = v as u8;
                }
            }
        }
        Preprocess::Sharpen => {
            // unsharp mask：原圖 + (原圖 - 模糊)。3x3 box blur 已足夠，不必高斯。
            let idx = |x: u32, y: u32| (y * w + x) as usize;
            let blurred: Vec<u8> = (0..h)
                .flat_map(|y| {
                    (0..w).map(move |x| (x, y))
                })
                .map(|(x, y)| {
                    let (mut sum, mut n) = (0u32, 0u32);
                    for dy in -1i32..=1 {
                        for dx in -1i32..=1 {
                            let (nx, ny) = (x as i32 + dx, y as i32 + dy);
                            if nx >= 0 && ny >= 0 && (nx as u32) < w && (ny as u32) < h {
                                sum += gray[idx(nx as u32, ny as u32)] as u32;
                                n += 1;
                            }
                        }
                    }
                    (sum / n) as u8
                })
                .collect();
            for i in 0..gray.len() {
                let sharp = gray[i] as i32 + (gray[i] as i32 - blurred[i] as i32);
                gray[i] = sharp.clamp(0, 255) as u8;
            }
        }
    }

    let mut out = RgbaImage::new(w, h);
    for (i, px) in out.pixels_mut().enumerate() {
        let g = gray[i];
        *px = image::Rgba([g, g, g, 255]);
    }
    out
}
