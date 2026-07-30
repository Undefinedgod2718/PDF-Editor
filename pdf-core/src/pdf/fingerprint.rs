//! P11 指紋 — 雙層：整份 SHA-256 完整性（step 1）＋每頁 perceptual hash
//! 內容指紋（step 2）。
//!
//! 不快取進 `DocMeta`：文件內容可被任何編輯管線改動（crop/compress/
//! protect/…call site 太多），快取需要在每一處變動後都記得invalidate，
//! 錯一處就回傳過期 hash。兩層都現算現回，比照 `protect::inspect` 每次
//! 讀 disk 現狀的作法。

use std::io::Read;
use std::path::Path;

use image::imageops::FilterType;
use pdfium_render::prelude::PdfDocument;
use sha2::{Digest, Sha256};

use super::ops::render_page_image;

/// 整份檔案的 SHA-256 hex digest。串流讀取，不整檔進記憶體一次 to_vec。
pub fn sha256_file(path: &Path) -> anyhow::Result<String> {
    let mut file = std::fs::File::open(path)?;
    let mut hasher = Sha256::new();
    let mut buf = [0u8; 64 * 1024];
    loop {
        let n = file.read(&mut buf)?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
    }
    Ok(format!("{:x}", hasher.finalize()))
}

/// 降採樣網格邊長：8x8 = 64 bit，剛好塞進一個 u64。
const PHASH_GRID: u32 = 8;
/// 渲染縮放：pixels-per-point。目標只是給降採樣一個夠穩定的輸入尺寸
/// （一般頁面 ~90-130px 寬），不必也不該用全解析度渲染只為了縮成 8x8。
const PHASH_RENDER_SCALE: f32 = 0.15;

/// 單頁 average-hash：渲染→降採樣灰階 8x8→跟 64 格平均值比較→組 64-bit。
/// 純 `image` crate 降採樣＋算術，不拉外部 phash crate（一致既有「全自研」
/// 原則）。抗重新編碼/輕微像素雜訊，但頁面內容實際變動（增刪文字/圖片）
/// 會讓多數 bit 翻轉——用來比對「這頁跟之前是不是同一份內容」，不是抓
/// 精確 diff（那是 P13 `compare` 的活）。
pub fn page_phash(doc: &PdfDocument, index: u16) -> anyhow::Result<u64> {
    let img = render_page_image(doc, index, PHASH_RENDER_SCALE)?;
    let small = image::imageops::resize(&img, PHASH_GRID, PHASH_GRID, FilterType::Triangle);
    let grays: Vec<u32> = small
        .pixels()
        .map(|p| {
            let [r, g, b, _] = p.0;
            (r as u32 + g as u32 + b as u32) / 3
        })
        .collect();
    Ok(hash_from_grays(&grays))
}

/// Bit `i` set iff grid cell `i`'s grayscale value is at/above the mean of
/// all cells. Split out from [`page_phash`] so the hashing arithmetic is
/// testable without a real PDFium render (no `pdfium.dll` binding needed).
fn hash_from_grays(grays: &[u32]) -> u64 {
    let avg: u32 = grays.iter().sum::<u32>() / grays.len() as u32;
    let mut hash: u64 = 0;
    for (i, &g) in grays.iter().enumerate() {
        if g >= avg {
            hash |= 1 << i;
        }
    }
    hash
}

/// 全文件每頁 phash，hex 編碼（16 字元 = 64 bit）方便塞進 JSON。
pub fn all_page_phashes(doc: &PdfDocument) -> anyhow::Result<Vec<String>> {
    let count = doc.pages().len();
    (0..count)
        .map(|i| page_phash(doc, i).map(|h| format!("{h:016x}")))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn known_vector() {
        let dir = std::env::temp_dir().join(format!("pdfcore-fp-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("hello.txt");
        std::fs::write(&path, b"hello world").unwrap();
        // sha256("hello world"), well-known test vector
        assert_eq!(
            sha256_file(&path).unwrap(),
            "b94d27b9934d3e08a52e52d7da7dabfac484efe37a5380ee9088f7ace2efcde9"
        );
    }

    #[test]
    fn hash_from_grays_bit_per_cell() {
        // 4 cells, avg = 127; cells >= avg set their bit (LSB-first index order).
        assert_eq!(hash_from_grays(&[0, 255, 0, 255]), 0b1010);
    }

    #[test]
    fn hash_from_grays_all_equal_is_all_ones() {
        // avg == every cell's value → every cell is ">= avg" → all bits set.
        assert_eq!(hash_from_grays(&[100, 100, 100, 100]), 0b1111);
    }

    #[test]
    fn hash_from_grays_stable_across_identical_input() {
        let a = vec![10, 200, 50, 90, 30, 220, 5, 60];
        assert_eq!(hash_from_grays(&a), hash_from_grays(&a));
    }
}
