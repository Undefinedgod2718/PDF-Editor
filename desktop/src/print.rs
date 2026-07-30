//! Windows GDI 列印（③）。
//!
//! 為什麼是這條路：Windows 上把 PDF 交給「系統列印」實際只有三種可能，
//! 兩種不能用——
//!
//! - `ShellExecute` 的 `print` 動詞會**委派給預設 PDF 程式**。我們自己就是
//!   （或想成為）預設程式，這是循環；不是的話等於叫 Chrome 幫我們印，行為不可控。
//! - WebView2 內建列印就是 `window.print()`，印的是畫在 canvas 上的頁面，
//!   跟網頁版同一個點陣品質問題，沒有比較好。
//! - 剩下這條：拿印表機的 DC，用**印表機自己的 DPI** 重新 render 再送出。
//!   文字仍然是點陣，但 300–600 DPI 下肉眼看不出來，而且頁範圍、份數、雙面
//!   都由驅動程式負責。
//!
//! 網頁版沒有這條路（瀏覽器碰不到印表機 DC），走 `window.print()`。

#![cfg(windows)]

use std::path::Path;

use pdf_editor_server::pdf::ops;
use pdfium_render::prelude::{PdfDocument, Pdfium};
use windows::core::{HSTRING, PCWSTR};
use windows::Win32::Foundation::HWND;
use windows::Win32::Graphics::Gdi::{
    CreateDCW, DeleteDC, GetDeviceCaps, StretchDIBits, BITMAPINFO, BITMAPINFOHEADER, BI_RGB,
    DIB_RGB_COLORS, HDC, HORZRES, LOGPIXELSX, SRCCOPY, VERTRES,
};
// 列印的 StartDoc/StartPage/... 在 windows-rs 裡歸在 Storage::Xps，不在 Graphics::Gdi。
use windows::Win32::Storage::Xps::{DOCINFOW, EndDoc, EndPage, StartDocW, StartPage};
use windows::Win32::UI::Controls::Dialogs::{
    PrintDlgW, PD_NOSELECTION, PD_RETURNDC, PRINTDLGW,
};

/// 渲染上限。600 DPI 的 A4 是 4960×7016 px＝約 139 MB RGBA，一頁一張還撐得住，
/// 但再往上（有些印表機報 1200）就會開始吃掉整個行程的記憶體，而且肉眼分不出來。
const MAX_PRINT_DPI: i32 = 600;

pub struct PrintJob<'a> {
    /// 0-based 頁碼；空的代表整份。
    pub pages: &'a [u16],
    /// 是否連註解一起印。
    pub annotations: bool,
    /// 指定印表機名稱＝不跳對話框直接印（自動化與「印到預設印表機」用）。
    /// `None` 會跳系統列印對話框，讓使用者選印表機與份數。
    pub printer: Option<&'a str>,
    /// 覆寫輸出連接埠。對「Microsoft Print to PDF」這類驅動而言就是輸出檔路徑，
    /// 給定之後它不會再跳存檔對話框——這也是這條路唯一能自動化驗證的方式。
    /// 只有搭配 `printer` 才有意義。
    pub output: Option<&'a str>,
}

/// 回傳實際送出的頁數。使用者在對話框按取消回 `Ok(0)`——那不是錯誤。
pub fn print_document(
    pdfium: &Pdfium,
    path: &Path,
    job: &PrintJob,
    owner: HWND,
) -> anyhow::Result<u32> {
    let hdc = match job.printer {
        Some(name) => create_printer_dc(name, job.output)?,
        None => match open_print_dialog(owner)? {
            Some(hdc) => hdc,
            None => return Ok(0),
        },
    };
    // 從這裡開始任何 early return 都得放掉 DC，所以包一層再統一收尾。
    let result = run_job(pdfium, path, job, hdc);
    unsafe {
        let _ = DeleteDC(hdc);
    }
    result
}

fn create_printer_dc(name: &str, output: Option<&str>) -> anyhow::Result<HDC> {
    let device = HSTRING::from(name);
    // driver 傳 NULL：讓 spooler 依裝置名自己查驅動。output 給定時當作連接埠覆寫。
    let out = output.map(HSTRING::from);
    let out_ptr = out.as_ref().map(|h| PCWSTR(h.as_ptr())).unwrap_or(PCWSTR::null());
    let hdc = unsafe { CreateDCW(PCWSTR::null(), PCWSTR(device.as_ptr()), out_ptr, None) };
    if hdc.is_invalid() {
        anyhow::bail!("cannot open printer '{name}'");
    }
    Ok(hdc)
}

fn open_print_dialog(owner: HWND) -> anyhow::Result<Option<HDC>> {
    let mut pd = PRINTDLGW {
        lStructSize: std::mem::size_of::<PRINTDLGW>() as u32,
        hwndOwner: owner,
        // PD_RETURNDC：對話框直接把選好的印表機 DC 給我們，不必自己拼 DEVMODE。
        // PD_NOSELECTION：文件裡沒有「選取範圍」的概念，把那個選項灰掉。
        Flags: PD_RETURNDC | PD_NOSELECTION,
        nCopies: 1,
        ..Default::default()
    };
    let ok = unsafe { PrintDlgW(&mut pd) };
    if !ok.as_bool() {
        // 使用者取消，或對話框開不起來。CommDlgExtendedError 可以分辨，但對呼叫端
        // 而言兩者都是「沒有印」，不值得為此多一種錯誤型別。
        return Ok(None);
    }
    if pd.hDC.is_invalid() {
        anyhow::bail!("print dialog returned no device context");
    }
    Ok(Some(pd.hDC))
}

fn run_job(pdfium: &Pdfium, path: &Path, job: &PrintJob, hdc: HDC) -> anyhow::Result<u32> {
    let doc = pdfium.load_pdf_from_file(path.to_string_lossy().as_ref(), None)?;
    let page_count = doc.pages().len();

    let wanted: Vec<u16> = if job.pages.is_empty() {
        (0..page_count).collect()
    } else {
        let mut v = job.pages.to_vec();
        v.sort_unstable();
        v.dedup();
        if let Some(bad) = v.iter().find(|p| **p >= page_count) {
            anyhow::bail!("page index {bad} out of range (0..{page_count})");
        }
        v
    };
    if wanted.is_empty() {
        return Ok(0);
    }

    let dpi = unsafe { GetDeviceCaps(Some(hdc), LOGPIXELSX) }.clamp(72, MAX_PRINT_DPI);
    // HORZRES/VERTRES 是**可列印區域**的像素，已經扣掉硬體邊界；把頁面等比例縮
    // 進去，比自己算邊界可靠。
    let dev_w = unsafe { GetDeviceCaps(Some(hdc), HORZRES) };
    let dev_h = unsafe { GetDeviceCaps(Some(hdc), VERTRES) };
    if dev_w <= 0 || dev_h <= 0 {
        anyhow::bail!("printer reports an empty printable area");
    }

    let doc_name = HSTRING::from(
        path.file_name()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_else(|| "PDF Editor".into()),
    );
    let docinfo = DOCINFOW {
        cbSize: std::mem::size_of::<DOCINFOW>() as i32,
        lpszDocName: PCWSTR(doc_name.as_ptr()),
        ..Default::default()
    };
    if unsafe { StartDocW(hdc, &docinfo) } <= 0 {
        anyhow::bail!("StartDoc failed; the print job was rejected by the spooler");
    }

    let mut printed = 0u32;
    for index in wanted {
        if let Err(e) = print_one_page(&doc, index, job.annotations, hdc, dpi, dev_w, dev_h) {
            // 中途失敗要收掉工作，否則會在 spooler 佇列裡留一份半殘的。
            unsafe {
                let _ = EndDoc(hdc);
            }
            return Err(e);
        }
        printed += 1;
    }
    if unsafe { EndDoc(hdc) } <= 0 {
        anyhow::bail!("EndDoc failed; the job may not have reached the printer");
    }
    Ok(printed)
}

fn print_one_page(
    doc: &PdfDocument,
    index: u16,
    annotations: bool,
    hdc: HDC,
    dpi: i32,
    dev_w: i32,
    dev_h: i32,
) -> anyhow::Result<()> {
    let page = doc.pages().get(index)?;
    let (pt_w, pt_h) = (page.width().value, page.height().value);
    drop(page);

    // 等比例縮進可列印區域，取較小的縮放比，避免任一邊被裁掉。
    let fit = (dev_w as f32 / pt_w).min(dev_h as f32 / pt_h);
    let target_w = (pt_w * fit).round().max(1.0) as i32;
    let target_h = (pt_h * fit).round().max(1.0) as i32;

    // 渲染解析度取「印表機 DPI」與「實際落在紙上的像素數」兩者較小者：印表機報
    // 600 DPI 但頁面只佔紙的一半時，用 600 render 出來的像素會被 StretchDIBits
    // 縮掉，白花記憶體。
    let scale = (dpi as f32 / 72.0).min(target_w as f32 / pt_w);
    let img = ops::render_page_image_with(doc, index, scale, annotations)?;
    let (src_w, src_h) = (img.width() as i32, img.height() as i32);

    // GDI 要 BGRA，且 BI_RGB 不看 alpha——PDFium 對沒有背景的頁面會給透明像素，
    // 直接送出去會印成黑色，所以先合成到白底再翻成 BGRX。
    let mut bgra = Vec::with_capacity((src_w * src_h * 4) as usize);
    for px in img.pixels() {
        let [r, g, b, a] = px.0;
        let over = |c: u8| -> u8 {
            let c = c as u32 * a as u32 + 255 * (255 - a as u32);
            (c / 255) as u8
        };
        bgra.extend_from_slice(&[over(b), over(g), over(r), 255]);
    }

    let header = BITMAPINFOHEADER {
        biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
        biWidth: src_w,
        // 負高度＝top-down。PDFium 給的是 top-down，不用自己翻列。
        biHeight: -src_h,
        biPlanes: 1,
        biBitCount: 32,
        biCompression: BI_RGB.0,
        ..Default::default()
    };
    let info = BITMAPINFO {
        bmiHeader: header,
        ..Default::default()
    };

    if unsafe { StartPage(hdc) } <= 0 {
        anyhow::bail!("StartPage failed on page {index}");
    }
    // 置中：剩餘空間各分一半。
    let ox = (dev_w - target_w) / 2;
    let oy = (dev_h - target_h) / 2;
    let blitted = unsafe {
        StretchDIBits(
            hdc,
            ox,
            oy,
            target_w,
            target_h,
            0,
            0,
            src_w,
            src_h,
            Some(bgra.as_ptr() as *const _),
            &info,
            DIB_RGB_COLORS,
            SRCCOPY,
        )
    };
    if blitted == 0 {
        unsafe {
            let _ = EndPage(hdc);
        }
        anyhow::bail!("StretchDIBits wrote nothing on page {index}");
    }
    if unsafe { EndPage(hdc) } <= 0 {
        anyhow::bail!("EndPage failed on page {index}");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 端到端印一份 PDF 出來檢查。用「Microsoft Print to PDF」＋輸出連接埠覆寫，
    /// 所以不會跳任何對話框，也不需要真的紙。
    ///
    /// 標 ignore 的理由：依賴這台機器上裝了那台印表機、以及 pdfium.dll 找得到。
    /// 跑法：
    ///   $env:PRINT_TEST_PDF="D:\...\some.pdf"; cargo test -p pdf-editor-desktop --
    ///     --ignored --nocapture print_to_pdf_file
    #[test]
    #[ignore]
    fn print_to_pdf_file() {
        let src = std::env::var("PRINT_TEST_PDF").expect("set PRINT_TEST_PDF");
        let out = std::env::temp_dir().join("pdf-editor-print-test.pdf");
        let _ = std::fs::remove_file(&out);

        let bindings = Pdfium::bind_to_library(Pdfium::pdfium_platform_library_name_at_path(
            &std::env::var("PDFIUM_DIR").unwrap_or_else(|_| "../server".into()),
        ))
        .or_else(|_| Pdfium::bind_to_system_library())
        .expect("no pdfium");
        let pdfium = Pdfium::new(bindings);

        let job = PrintJob {
            pages: &[0],
            annotations: true,
            printer: Some("Microsoft Print to PDF"),
            output: Some(out.to_str().unwrap()),
        };
        let printed = print_document(&pdfium, Path::new(&src), &job, HWND::default())
            .expect("print failed");
        assert_eq!(printed, 1, "expected one page to be sent");

        // 驅動是非同步寫檔的，給它一點時間落地。
        for _ in 0..40 {
            if std::fs::metadata(&out).map(|m| m.len() > 0).unwrap_or(false) {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(250));
        }
        let bytes = std::fs::read(&out).expect("no output file produced");
        println!("printed {} bytes to {}", bytes.len(), out.display());
        assert!(bytes.starts_with(b"%PDF"), "output is not a PDF");
        assert!(bytes.len() > 5_000, "output suspiciously small: {} bytes", bytes.len());
    }
}
