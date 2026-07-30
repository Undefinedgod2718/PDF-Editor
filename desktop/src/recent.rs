//! 「最近使用」清單（桌面版限定）。
//!
//! 只記錄有真實本機路徑的開檔（`open_by_path`／`open_dialog` 成功時），
//! 存在 Tauri app config dir 的 `recent.json`，縮圖另存 `thumbs/<key>.png`。
//! 這支模組刻意跟 Tauri／axum 解耦：核心資料結構與檔案 I/O
//! （[`RecentStore`]）只吃路徑參數、不碰 `AppHandle`，方便單元測試；
//! 只有模組底部的 handler 們需要 `local_api::APP_HANDLE` 去問
//! app config dir 在哪、以及 `SharedState.engine` 去產生縮圖。
//!
//! 全模組的第一原則：這是錦上添花的功能，壞掉不能連累「開檔」這個
//! 核心操作。清單讀寫、縮圖產生一律 best-effort——失敗只記 log，
//! 絕不 panic、絕不讓呼叫端的開檔請求跟著失敗。

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};

use axum::extract::Path as AxPath;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use tauri::{AppHandle, Manager};
use uuid::Uuid;

use pdf_editor_server::{pdf, SharedState};

/// 未加星號項目的上限；加星號的完全不受這個上限管，也永遠不會被自動淘汰。
const UNSTARRED_CAP: usize = 20;

/// 縮圖目標寬度約 120px。`ops::render_page` 吃的是「每 PDF point 幾像素」
/// 的 scale，不是目標寬度；PDF point 慣例 72/inch，A4/Letter 寬度落在
/// 595~612pt 之間，0.2 大約落在 119~122px，夠接近需求，不值得為了精確
/// 120px 去改 pdf-core 的 render 介面（規格明講不要另外寫 renderer）。
const THUMB_SCALE: f32 = 0.2;

fn err(status: StatusCode, e: impl std::fmt::Display) -> Response {
    (status, Json(serde_json::json!({ "error": e.to_string() }))).into_response()
}

// ---------------------------------------------------------------------
// 資料結構
// ---------------------------------------------------------------------

#[derive(Serialize, Deserialize, Clone)]
struct RecentEntry {
    /// 使用者看得懂的原始路徑（給 UI 顯示用）。可能跟磁碟上的真實路徑
    /// 有大小寫或 `..`/`.` 差異——這些正規化只用來算 identity/縮圖 key，
    /// 顯示一律用這個欄位保留原樣。
    path: String,
    filename: String,
    /// RFC3339、固定到秒、UTC（例如 `2026-07-27T04:15:00Z`）。刻意用
    /// 固定寬度格式，字串排序＝時間排序，省得為了排序再 parse 一次。
    last_opened: String,
    #[serde(default)]
    starred: bool,
}

#[derive(Serialize, Deserialize)]
struct RecentData {
    #[serde(default = "default_version")]
    version: u32,
    #[serde(default)]
    entries: Vec<RecentEntry>,
}

fn default_version() -> u32 {
    1
}

impl Default for RecentData {
    fn default() -> Self {
        Self {
            version: default_version(),
            entries: Vec::new(),
        }
    }
}

impl RecentData {
    /// 讀不到／解析失敗都當成空清單——「檔案不存在」是正常的初次啟動，
    /// 「解析失敗」是損毀，兩者都不該讓 app 打不開檔案，所以行為一致：
    /// 記 log（僅損毀情況）然後回傳空清單，讓後續操作從乾淨狀態繼續。
    fn load(path: &Path) -> Self {
        match std::fs::read_to_string(path) {
            Ok(text) => serde_json::from_str::<RecentData>(&text).unwrap_or_else(|e| {
                tracing::warn!(
                    "recent.json 解析失敗（{}），視為空清單: {e}",
                    path.display()
                );
                RecentData::default()
            }),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => RecentData::default(),
            Err(e) => {
                tracing::warn!("recent.json 讀取失敗（{}）: {e}", path.display());
                RecentData::default()
            }
        }
    }

    /// 原子寫：同目錄先寫 `.tmp` 再 rename，斷電最多留下 tmp 殘檔，絕不會
    /// 讓 `recent.json` 本體停在寫一半的狀態。Windows 上 rename 蓋既有檔
    /// 偶爾會被拒（防毒/索引程式打開中），退回先刪再 rename——跟
    /// `pdf-core::storage::atomic_copy` 同一招。
    fn save_atomic(&self, path: &Path) -> anyhow::Result<()> {
        let dir = path.parent().unwrap_or_else(|| Path::new("."));
        std::fs::create_dir_all(dir)?;
        let tmp = dir.join(format!(
            "{}.tmp-{}",
            path.file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_else(|| "recent.json".into()),
            Uuid::new_v4().simple()
        ));
        std::fs::write(&tmp, serde_json::to_string_pretty(self)?)?;
        match std::fs::rename(&tmp, path) {
            Ok(()) => Ok(()),
            Err(e) if cfg!(windows) => {
                tracing::warn!("recent.json 原子 rename 失敗（{e}），退回先刪再 rename");
                let _ = std::fs::remove_file(path);
                std::fs::rename(&tmp, path).inspect_err(|_| {
                    let _ = std::fs::remove_file(&tmp);
                })?;
                Ok(())
            }
            Err(e) => {
                let _ = std::fs::remove_file(&tmp);
                Err(e.into())
            }
        }
    }

    /// Upsert：同一份文件（同 identity key）重開只更新 `last_opened`／
    /// 路徑顯示字串／檔名，不會多插一筆。回傳 key 給呼叫端拿去對縮圖檔名。
    fn upsert_open(&mut self, display_path: String, filename: String, when: String) -> String {
        let key = compute_key(Path::new(&display_path));
        match self
            .entries
            .iter_mut()
            .find(|e| compute_key(Path::new(&e.path)) == key)
        {
            Some(existing) => {
                existing.path = display_path;
                existing.filename = filename;
                existing.last_opened = when;
            }
            None => self.entries.push(RecentEntry {
                path: display_path,
                filename,
                last_opened: when,
                starred: false,
            }),
        }
        self.enforce_cap();
        key
    }

    /// 只數未加星號的項目；超過上限就砍掉最舊的（`last_opened` 最小）
    /// 未加星號項目，加星號的完全不列入計算也不會被砍。
    fn enforce_cap(&mut self) {
        let unstarred_count = self.entries.iter().filter(|e| !e.starred).count();
        if unstarred_count <= UNSTARRED_CAP {
            return;
        }
        let overflow = unstarred_count - UNSTARRED_CAP;
        let evict: HashSet<usize> = {
            let entries = &self.entries;
            let mut idx: Vec<usize> = entries
                .iter()
                .enumerate()
                .filter(|(_, e)| !e.starred)
                .map(|(i, _)| i)
                .collect();
            idx.sort_by(|&a, &b| entries[a].last_opened.cmp(&entries[b].last_opened));
            idx.into_iter().take(overflow).collect()
        };
        let mut i = 0usize;
        self.entries.retain(|_| {
            let keep = !evict.contains(&i);
            i += 1;
            keep
        });
    }
}

/// 路徑正規化：只做「絕對化 + 解 `.`/`..` + 分隔符統一 + 大小寫摺疊」的
/// 純字串運算，刻意不呼叫 `fs::canonicalize`。原因：canonicalize 需要
/// 檔案真的存在才能解析（含 symlink），可是「加星號／移除／清除」這幾支
/// API 收到的是使用者從清單裡點的路徑字串，這時檔案可能已經被移走或刪除
/// ——如果正規化依賴檔案是否存在，同一個字串在檔案還在跟檔案被刪之後
/// 會算出不同的 key，identity 比對就會悄悄壞掉。純字串正規化不管檔案
/// 是否存在都給同一個答案，犧牲一點 symlink/junction 去重的精確度，換
/// 來「一定不會把同一份文件在清單裡拆成兩筆／找不到要刪除的那筆」的
/// 穩定性——規格本身也把「絕對化 + 大小寫摺疊」列為可接受的最低要求。
fn normalize_path(path: &Path) -> String {
    let absolute = if path.is_absolute() {
        path.to_path_buf()
    } else {
        std::env::current_dir()
            .map(|cwd| cwd.join(path))
            .unwrap_or_else(|_| path.to_path_buf())
    };
    let mut cleaned = PathBuf::new();
    for component in absolute.components() {
        match component {
            std::path::Component::CurDir => {}
            std::path::Component::ParentDir => {
                cleaned.pop();
            }
            other => cleaned.push(other.as_os_str()),
        }
    }
    cleaned
        .to_string_lossy()
        .replace('/', "\\")
        .to_lowercase()
}

/// Identity key＝縮圖檔名 key：正規化路徑的 SHA-256，取前 16 hex
/// 字元（8 bytes）。兩個用途共用同一個值，省一次雜湊、也保證「這篇
/// 文件的縮圖」跟「這篇文件在清單裡的身分」永遠對得上。
fn compute_key(path: &Path) -> String {
    let normalized = normalize_path(path);
    let digest = Sha256::digest(normalized.as_bytes());
    digest.iter().take(8).map(|b| format!("{b:02x}")).collect()
}

fn now_rfc3339() -> String {
    chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true)
}

// ---------------------------------------------------------------------
// Store：Mutex 包住記憶體快取 + 落盤，兩個並發開檔請求不會互相蓋掉對方
// 剛寫的那筆（read-modify-write 全程持鎖）。
// ---------------------------------------------------------------------

struct RecentStore {
    file: PathBuf,
    thumbs_dir: PathBuf,
    data: Mutex<RecentData>,
}

impl RecentStore {
    fn new(file: PathBuf, thumbs_dir: PathBuf) -> Self {
        if let Err(e) = std::fs::create_dir_all(&thumbs_dir) {
            tracing::warn!("縮圖目錄建立失敗（{}）: {e}", thumbs_dir.display());
        }
        let data = RecentData::load(&file);
        Self {
            file,
            thumbs_dir,
            data: Mutex::new(data),
        }
    }

    fn thumb_path(&self, key: &str) -> PathBuf {
        self.thumbs_dir.join(format!("{key}.png"))
    }

    /// 鎖中斯出，poison 就直接拿內容繼續用——這個 Mutex 保護的臨界區只有
    /// 同步的記憶體操作跟檔案 I/O，不會 panic，但萬一真的 panic 過，
    /// 「recent 清單壞掉」也不該連坐讓後面的開檔跟著失敗。
    fn lock(&self) -> std::sync::MutexGuard<'_, RecentData> {
        self.data.lock().unwrap_or_else(|poison| poison.into_inner())
    }

    fn record_open(&self, display_path: String, filename: String) -> String {
        let mut data = self.lock();
        let key = data.upsert_open(display_path, filename, now_rfc3339());
        if let Err(e) = data.save_atomic(&self.file) {
            tracing::warn!("recent.json 寫入失敗（不影響開檔）: {e}");
        }
        key
    }

    /// 依 `last_opened` 新到舊排序；星號分組是前端的事（規格明講）。
    fn list(&self) -> Vec<RecentEntry> {
        let mut entries = self.lock().entries.clone();
        entries.sort_by(|a, b| b.last_opened.cmp(&a.last_opened));
        entries
    }

    fn set_starred(&self, raw_path: &str, starred: bool) -> bool {
        let key = compute_key(Path::new(raw_path));
        let mut data = self.lock();
        let Some(entry) = data
            .entries
            .iter_mut()
            .find(|e| compute_key(Path::new(&e.path)) == key)
        else {
            return false;
        };
        entry.starred = starred;
        if let Err(e) = data.save_atomic(&self.file) {
            tracing::warn!("recent.json 寫入失敗（不影響操作）: {e}");
        }
        true
    }

    fn remove(&self, raw_path: &str) -> bool {
        let key = compute_key(Path::new(raw_path));
        let mut data = self.lock();
        let before = data.entries.len();
        data.entries
            .retain(|e| compute_key(Path::new(&e.path)) != key);
        let removed = data.entries.len() != before;
        if removed {
            if let Err(e) = data.save_atomic(&self.file) {
                tracing::warn!("recent.json 寫入失敗（不影響操作）: {e}");
            }
            // 縮圖跟著清掉，找不到就算了（本來就沒有或已經被清過）。
            let _ = std::fs::remove_file(self.thumb_path(&key));
        }
        removed
    }

    /// 只清未加星號的；加星號的原地不動。
    fn clear_unstarred(&self) {
        let mut data = self.lock();
        let mut removed_keys = Vec::new();
        data.entries.retain(|e| {
            if e.starred {
                true
            } else {
                removed_keys.push(compute_key(Path::new(&e.path)));
                false
            }
        });
        if let Err(e) = data.save_atomic(&self.file) {
            tracing::warn!("recent.json 寫入失敗（不影響操作）: {e}");
        }
        for key in removed_keys {
            let _ = std::fs::remove_file(self.thumb_path(&key));
        }
    }
}

static STORE: OnceLock<RecentStore> = OnceLock::new();

fn get_store(app: &AppHandle) -> &'static RecentStore {
    STORE.get_or_init(|| {
        let base = app.path().app_config_dir().unwrap_or_else(|e| {
            tracing::warn!("app_config_dir 取得失敗，最近清單退回暫存目錄: {e}");
            std::env::temp_dir().join("pdf-editor-recent-fallback")
        });
        RecentStore::new(base.join("recent.json"), base.join("thumbs"))
    })
}

// ---------------------------------------------------------------------
// 開檔時記錄（給 local_api.rs 的 open_by_path／open_dialog 呼叫）
// ---------------------------------------------------------------------

/// 只在開檔成功後呼叫。整支函式沒有會 `?` 往外拋的錯誤——所有失敗
/// 路徑都在內部處理成 log，呼叫端拿到的永遠是 `()`。
pub async fn record_open(state: &SharedState, source_path: &Path, doc_id: Uuid) {
    let Some(app) = crate::local_api::APP_HANDLE.get() else {
        // 理論上 setup() 一定先於任何請求跑完；真的沒有就靜靜跳過，
        // 「記不到最近清單」不該變成錯誤訊息干擾使用者。
        return;
    };
    let store = get_store(app);
    let filename = source_path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| "document.pdf".into());
    let display_path = source_path.to_string_lossy().into_owned();
    let key = store.record_open(display_path, filename);

    let thumb_path = store.thumb_path(&key);
    if thumb_is_fresh(&thumb_path, source_path) {
        return;
    }
    let working_copy = state.storage.pdf_path(doc_id);
    let render = state
        .engine
        .run(move |pdfium, cache| {
            let doc = cache.open(pdfium, &working_copy)?;
            pdf::ops::render_page(doc, 0, THUMB_SCALE)
        })
        .await;
    match render {
        Ok(png) => {
            if let Err(e) = write_thumb_atomic(&thumb_path, &png) {
                tracing::warn!("縮圖寫入失敗（不影響開檔）: {e}");
            }
        }
        Err(e) => tracing::warn!("縮圖產生失敗（不影響開檔）: {e}"),
    }
}

/// 縮圖已經比來源檔案新（mtime 晚於或等於來源）就不必重算——不需要另外
/// 記一個「上次產生時來源 mtime 是多少」，直接比兩個檔案現在的 mtime
/// 就夠：只要縮圖是在來源最後一次修改「之後」寫的，內容必然是新的。
fn thumb_is_fresh(thumb_path: &Path, source_path: &Path) -> bool {
    let (Ok(thumb_meta), Ok(source_meta)) =
        (std::fs::metadata(thumb_path), std::fs::metadata(source_path))
    else {
        return false;
    };
    matches!(
        (thumb_meta.modified(), source_meta.modified()),
        (Ok(t), Ok(s)) if t >= s
    )
}

fn write_thumb_atomic(dest: &Path, png_bytes: &[u8]) -> anyhow::Result<()> {
    let dir = dest.parent().unwrap_or_else(|| Path::new("."));
    std::fs::create_dir_all(dir)?;
    let tmp = dir.join(format!(".thumb-{}.tmp", Uuid::new_v4().simple()));
    std::fs::write(&tmp, png_bytes)?;
    match std::fs::rename(&tmp, dest) {
        Ok(()) => Ok(()),
        Err(_) if cfg!(windows) => {
            let _ = std::fs::remove_file(dest);
            std::fs::rename(&tmp, dest).inspect_err(|_| {
                let _ = std::fs::remove_file(&tmp);
            })?;
            Ok(())
        }
        Err(e) => {
            let _ = std::fs::remove_file(&tmp);
            Err(e.into())
        }
    }
}

// ---------------------------------------------------------------------
// HTTP routes
// ---------------------------------------------------------------------

pub fn router() -> Router<SharedState> {
    Router::new()
        .route("/api/local/recent", get(list_recent))
        .route("/api/local/recent/star", post(star))
        .route("/api/local/recent/remove", post(remove))
        .route("/api/local/recent/clear", post(clear))
        .route("/api/local/recent/thumb/{key}", get(thumb))
}

#[derive(Serialize)]
struct ClientEntry {
    path: String,
    filename: String,
    last_opened: String,
    starred: bool,
    exists: bool,
    size: Option<u64>,
    thumb_key: Option<String>,
}

/// `exists`／`size` 現場算，絕不落盤——落盤的話，檔案在別的程式裡被搬走
/// 或改大小就立刻跟現實不符了。找不到的文件留在清單裡（不自動移除），
/// 前端灰掉並問使用者要不要移除，是規格明講的行為。
async fn list_recent() -> Response {
    let Some(app) = crate::local_api::APP_HANDLE.get() else {
        return Json(serde_json::json!({ "entries": [] })).into_response();
    };
    let store = get_store(app);
    let client: Vec<ClientEntry> = store
        .list()
        .into_iter()
        .map(|e| {
            let key = compute_key(Path::new(&e.path));
            let p = Path::new(&e.path);
            let exists = p.exists();
            let size = if exists {
                std::fs::metadata(p).ok().map(|m| m.len())
            } else {
                None
            };
            let thumb_key = if store.thumb_path(&key).exists() {
                Some(key)
            } else {
                None
            };
            ClientEntry {
                path: e.path,
                filename: e.filename,
                last_opened: e.last_opened,
                starred: e.starred,
                exists,
                size,
                thumb_key,
            }
        })
        .collect();
    Json(serde_json::json!({ "entries": client })).into_response()
}

#[derive(Deserialize)]
struct StarReq {
    path: String,
    starred: bool,
}

async fn star(Json(req): Json<StarReq>) -> Response {
    let Some(app) = crate::local_api::APP_HANDLE.get() else {
        return err(StatusCode::SERVICE_UNAVAILABLE, "app handle not ready");
    };
    if get_store(app).set_starred(&req.path, req.starred) {
        StatusCode::NO_CONTENT.into_response()
    } else {
        err(StatusCode::NOT_FOUND, "recent entry not found")
    }
}

#[derive(Deserialize)]
struct RemoveReq {
    path: String,
}

async fn remove(Json(req): Json<RemoveReq>) -> Response {
    let Some(app) = crate::local_api::APP_HANDLE.get() else {
        return err(StatusCode::SERVICE_UNAVAILABLE, "app handle not ready");
    };
    if get_store(app).remove(&req.path) {
        StatusCode::NO_CONTENT.into_response()
    } else {
        err(StatusCode::NOT_FOUND, "recent entry not found")
    }
}

async fn clear() -> Response {
    let Some(app) = crate::local_api::APP_HANDLE.get() else {
        return err(StatusCode::SERVICE_UNAVAILABLE, "app handle not ready");
    };
    get_store(app).clear_unstarred();
    StatusCode::NO_CONTENT.into_response()
}

/// key 來自 URL、直接拿去拼檔案路徑，先驗證「16 個 hex 字元」再碰檔案
/// 系統——不然 `../../whatever` 這種 key 就是任意檔案讀取。
async fn thumb(AxPath(key): AxPath<String>) -> Response {
    if key.len() != 16 || !key.bytes().all(|b| b.is_ascii_hexdigit()) {
        return err(StatusCode::BAD_REQUEST, "invalid thumbnail key");
    }
    let Some(app) = crate::local_api::APP_HANDLE.get() else {
        return err(StatusCode::SERVICE_UNAVAILABLE, "app handle not ready");
    };
    let path = get_store(app).thumb_path(&key);
    match tokio::fs::read(&path).await {
        Ok(bytes) => (
            [(axum::http::header::CONTENT_TYPE, "image/png")],
            bytes,
        )
            .into_response(),
        Err(_) => StatusCode::NOT_FOUND.into_response(),
    }
}

// ---------------------------------------------------------------------
// 單元測試：只測不需要 Tauri/PDFium 的核心邏輯（RecentStore 的同步部分）。
// 縮圖產生（`record_open` 裡呼叫 engine）需要真的 PDFium binding 跟
// Tauri AppHandle，這支模組刻意讓它們跟這裡的邏輯解耦，所以不在這裡測，
// 也測不到——這是設計取捨，見檔頭註解。
// ---------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    fn tmpdir(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("pdfed-recent-test-{tag}-{}", Uuid::new_v4()));
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    fn store(tag: &str) -> RecentStore {
        let dir = tmpdir(tag);
        RecentStore::new(dir.join("recent.json"), dir.join("thumbs"))
    }

    #[test]
    fn reopen_same_path_updates_not_appends() {
        let s = store("reopen");
        let k1 = s.record_open(r"D:\Docs\a.pdf".into(), "a.pdf".into());
        let k2 = s.record_open(r"D:\Docs\a.pdf".into(), "a.pdf".into());
        assert_eq!(k1, k2);
        let entries = s.list();
        assert_eq!(entries.len(), 1, "重開同一份文件不該多一筆");
    }

    #[test]
    fn reopen_is_case_insensitive_on_display_path() {
        let s = store("case");
        s.record_open(r"D:\Docs\A.pdf".into(), "A.pdf".into());
        s.record_open(r"d:\docs\a.pdf".into(), "a.pdf".into());
        assert_eq!(s.list().len(), 1, "Windows 路徑大小寫視為同一份文件");
    }

    /// 直接灌合成的 `last_opened` 進記憶體結構，不靠 `sleep` 製造時間差
    /// ——`record_open` 一律蓋成「現在」，真的睡出秒級差異會讓測試變慢
    /// 又脆弱。這裡測的是 `enforce_cap` 這個純函式本身的邏輯。
    #[test]
    fn cap_evicts_only_unstarred_oldest_first() {
        let s = store("cap");
        {
            let mut data = s.lock();
            data.entries.push(RecentEntry {
                path: r"D:\starred.pdf".into(),
                filename: "starred.pdf".into(),
                last_opened: "2020-01-01T00:00:00Z".into(),
                starred: true,
            });
            for i in 0..25 {
                data.entries.push(RecentEntry {
                    path: format!(r"D:\Docs\{i}.pdf"),
                    filename: format!("{i}.pdf"),
                    // i 越大越晚開啟。
                    last_opened: format!("2024-01-01T00:{i:02}:00Z"),
                    starred: false,
                });
            }
            data.enforce_cap();
        }

        let entries = s.list();
        let unstarred: Vec<_> = entries.iter().filter(|e| !e.starred).collect();
        assert_eq!(unstarred.len(), 20, "未加星號的項目應該被砍到剩 20 筆");
        assert!(
            entries.iter().any(|e| e.starred),
            "加星號的項目不該被自動淘汰"
        );
        // 最舊的 5 筆（0.pdf ~ 4.pdf）該被砍掉，較新的留下。
        for i in 0..5 {
            assert!(
                !unstarred.iter().any(|e| e.filename == format!("{i}.pdf")),
                "{i}.pdf 是最舊的幾筆之一，應該被淘汰"
            );
        }
        assert!(unstarred.iter().any(|e| e.filename == "24.pdf"));
    }

    #[test]
    fn clear_keeps_starred_removes_rest() {
        let s = store("clear");
        s.record_open(r"D:\keep.pdf".into(), "keep.pdf".into());
        s.record_open(r"D:\drop.pdf".into(), "drop.pdf".into());
        assert!(s.set_starred(r"D:\keep.pdf", true));

        s.clear_unstarred();

        let entries = s.list();
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].filename, "keep.pdf");
        assert!(entries[0].starred);
    }

    #[test]
    fn corrupt_json_loads_as_empty_not_panic() {
        let dir = tmpdir("corrupt");
        let file = dir.join("recent.json");
        std::fs::write(&file, b"{ not valid json at all").unwrap();

        let data = RecentData::load(&file);
        assert_eq!(data.entries.len(), 0);
        assert_eq!(data.version, 1);
    }

    #[test]
    fn missing_json_loads_as_empty() {
        let dir = tmpdir("missing");
        let file = dir.join("does-not-exist.json");
        let data = RecentData::load(&file);
        assert_eq!(data.entries.len(), 0);
    }

    #[test]
    fn remove_deletes_matching_entry_and_reports_not_found_otherwise() {
        let s = store("remove");
        s.record_open(r"D:\a.pdf".into(), "a.pdf".into());
        assert!(!s.remove(r"D:\nope.pdf"));
        assert!(s.remove(r"D:\a.pdf"));
        assert_eq!(s.list().len(), 0);
    }

    #[test]
    fn thumb_key_is_16_lowercase_hex_chars() {
        let key = compute_key(Path::new(r"D:\Docs\a.pdf"));
        assert_eq!(key.len(), 16);
        assert!(key.bytes().all(|b| b.is_ascii_hexdigit()));
        assert_eq!(key, key.to_lowercase());
    }
}
