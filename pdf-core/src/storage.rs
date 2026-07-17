use std::path::{Path, PathBuf};

use dashmap::DashMap;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

#[derive(Clone, Serialize, Deserialize)]
pub struct DocMeta {
    pub id: Uuid,
    pub filename: String,
    pub size: u64,
    /// Bumped on every content mutation. Clients put it in render URLs
    /// (`?v=N`) so page images can be cached as immutable.
    #[serde(default)]
    pub revision: u64,
    /// SHA-256 hex digest of the owner password set by `/protect`, or `None`
    /// if the document isn't protected (or was protected outside this app).
    /// A PDF with an empty user password auto-decrypts on load in any
    /// reader (including our own `lopdf`), so the on-disk `/Encrypt`
    /// dictionary alone can't validate the owner password later — this
    /// hash is what `/unprotect` actually checks the caller's password
    /// against. Never return this over the API; use [`DocMeta::for_client`].
    #[serde(default)]
    pub protection_hash: Option<String>,
    /// Path session（ADR-004，桌面版）：文件來源檔路徑。編輯一律作用在
    /// 工作副本 `{id}.pdf`，「儲存」才寫回這裡。Upload session 為 `None`。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub origin: Option<PathBuf>,
    /// 開檔/上次儲存時來源檔的 mtime（unix 秒）。儲存前比對，偵測
    /// 外部程式同時改檔（ADR-004）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub origin_mtime: Option<u64>,
    /// 上次寫回來源檔時的 revision。前端 dirty = revision != saved_revision。
    /// Upload session 恆為 0（無「儲存」語意）。
    #[serde(default)]
    pub saved_revision: u64,
}

impl DocMeta {
    /// Drop secrets before serializing metadata into an HTTP response.
    pub fn for_client(mut self) -> Self {
        self.protection_hash = None;
        self
    }
}

#[derive(Clone, Serialize, Deserialize)]
pub struct StampMeta {
    pub id: Uuid,
    pub filename: String,
    /// Pixel dimensions of the uploaded image.
    pub width: u32,
    pub height: u32,
}

pub struct Storage {
    root: PathBuf,
    stamps_root: PathBuf,
    docs: DashMap<Uuid, DocMeta>,
    stamps: DashMap<Uuid, StampMeta>,
}

impl Storage {
    pub fn new(root: impl AsRef<Path>) -> anyhow::Result<Self> {
        let root = root.as_ref().to_path_buf();
        let stamps_root = root.join("stamps");
        std::fs::create_dir_all(&root)?;
        std::fs::create_dir_all(&stamps_root)?;
        let docs = DashMap::new();
        // Reload metadata sidecars so documents survive a server restart.
        for entry in std::fs::read_dir(&root)? {
            let path = entry?.path();
            if path.extension().is_some_and(|e| e == "json") {
                if let Ok(text) = std::fs::read_to_string(&path) {
                    if let Ok(meta) = serde_json::from_str::<DocMeta>(&text) {
                        docs.insert(meta.id, meta);
                    }
                }
            }
        }
        let stamps = DashMap::new();
        for entry in std::fs::read_dir(&stamps_root)? {
            let path = entry?.path();
            if path.extension().is_some_and(|e| e == "json") {
                if let Ok(text) = std::fs::read_to_string(&path) {
                    if let Ok(meta) = serde_json::from_str::<StampMeta>(&text) {
                        stamps.insert(meta.id, meta);
                    }
                }
            }
        }
        Ok(Self {
            root,
            stamps_root,
            docs,
            stamps,
        })
    }

    pub fn pdf_path(&self, id: Uuid) -> PathBuf {
        self.root.join(format!("{id}.pdf"))
    }

    fn meta_path(&self, id: Uuid) -> PathBuf {
        self.root.join(format!("{id}.json"))
    }

    /// Persist a new PDF and its sidecar metadata. `protection_hash` is written
    /// in the same sidecar write as the rest of `DocMeta` so a crash between
    /// PDF write and hash update cannot leave a protected file without a
    /// verifier (which would force `/unprotect` to refuse — safer than a
    /// password bypass, but still a stuck document).
    pub fn save(
        &self,
        filename: String,
        bytes: &[u8],
        protection_hash: Option<String>,
    ) -> anyhow::Result<DocMeta> {
        let id = Uuid::new_v4();
        std::fs::write(self.pdf_path(id), bytes)?;
        let meta = DocMeta {
            id,
            filename,
            size: bytes.len() as u64,
            revision: 0,
            protection_hash,
            origin: None,
            origin_mtime: None,
            saved_revision: 0,
        };
        std::fs::write(self.meta_path(id), serde_json::to_string(&meta)?)?;
        self.docs.insert(id, meta.clone());
        Ok(meta)
    }

    // ---- path sessions（ADR-004，桌面版）----

    /// 由本機路徑建立 session：讀入來源檔做工作副本，之後所有編輯管線
    /// 與 upload session 完全相同；差別只在 meta 記住 origin 供寫回。
    pub fn open_path(&self, path: impl AsRef<Path>) -> anyhow::Result<DocMeta> {
        let path = std::fs::canonicalize(path.as_ref())?;
        let bytes = std::fs::read(&path)?;
        let filename = path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| "document.pdf".into());
        let id = Uuid::new_v4();
        std::fs::write(self.pdf_path(id), &bytes)?;
        let meta = DocMeta {
            id,
            filename,
            size: bytes.len() as u64,
            revision: 0,
            protection_hash: None,
            origin_mtime: file_mtime(&path),
            origin: Some(path),
            saved_revision: 0,
        };
        std::fs::write(self.meta_path(id), serde_json::to_string(&meta)?)?;
        self.docs.insert(id, meta.clone());
        Ok(meta)
    }

    /// 把工作副本寫回來源檔（Ctrl+S）。原子寫：同目錄 temp + rename，
    /// 防寫到一半斷電壞原檔。`force=false` 時若來源檔 mtime 與開檔/上次
    /// 儲存時不符（外部程式改過）→ 拒絕，讓前端問使用者。
    pub fn save_to_origin(&self, id: Uuid, force: bool) -> anyhow::Result<DocMeta> {
        let meta = self
            .get(id)
            .ok_or_else(|| anyhow::anyhow!("document not found"))?;
        let origin = meta
            .origin
            .clone()
            .ok_or_else(|| anyhow::anyhow!("document has no origin path (upload session)"))?;
        if !force {
            let current = file_mtime(&origin);
            if current != meta.origin_mtime {
                anyhow::bail!("origin file changed on disk since open/last save");
            }
        }
        atomic_copy(&self.pdf_path(id), &origin)?;
        self.finish_save(id, origin)
    }

    /// 另存新檔：工作副本寫到新路徑，session 的 origin 改指過去。
    pub fn save_as(&self, id: Uuid, dest: impl AsRef<Path>) -> anyhow::Result<DocMeta> {
        let dest = dest.as_ref().to_path_buf();
        atomic_copy(&self.pdf_path(id), &dest)?;
        // canonicalize 需在檔案存在後
        let dest = std::fs::canonicalize(&dest).unwrap_or(dest);
        self.finish_save(id, dest)
    }

    /// 寫回成功後更新 meta：origin/mtime/saved_revision + 檔名對齊新路徑。
    fn finish_save(&self, id: Uuid, origin: PathBuf) -> anyhow::Result<DocMeta> {
        let snapshot = {
            let mut meta = self
                .docs
                .get_mut(&id)
                .ok_or_else(|| anyhow::anyhow!("document not found"))?;
            meta.origin_mtime = file_mtime(&origin);
            meta.filename = origin
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_else(|| meta.filename.clone());
            meta.origin = Some(origin);
            meta.saved_revision = meta.revision;
            meta.clone()
        };
        std::fs::write(self.meta_path(id), serde_json::to_string(&snapshot)?)?;
        Ok(snapshot)
    }

    pub fn get(&self, id: Uuid) -> Option<DocMeta> {
        self.docs.get(&id).map(|m| m.clone())
    }

    /// Increment a document's revision and persist it to the sidecar.
    /// Call after every successful content mutation.
    pub fn bump_revision(&self, id: Uuid) -> anyhow::Result<u64> {
        let snapshot = {
            let mut meta = self
                .docs
                .get_mut(&id)
                .ok_or_else(|| anyhow::anyhow!("document not found"))?;
            meta.revision += 1;
            meta.clone()
        }; // release the map lock before touching the filesystem
        std::fs::write(self.meta_path(id), serde_json::to_string(&snapshot)?)?;
        Ok(snapshot.revision)
    }

    pub fn list(&self) -> Vec<DocMeta> {
        self.docs.iter().map(|m| m.clone()).collect()
    }

    // ---- stamps ----

    pub fn stamp_path(&self, id: Uuid) -> PathBuf {
        self.stamps_root.join(format!("{id}.png"))
    }

    fn stamp_meta_path(&self, id: Uuid) -> PathBuf {
        self.stamps_root.join(format!("{id}.json"))
    }

    pub fn save_stamp(
        &self,
        filename: String,
        width: u32,
        height: u32,
        png_bytes: &[u8],
    ) -> anyhow::Result<StampMeta> {
        let id = Uuid::new_v4();
        std::fs::write(self.stamp_path(id), png_bytes)?;
        let meta = StampMeta {
            id,
            filename,
            width,
            height,
        };
        std::fs::write(self.stamp_meta_path(id), serde_json::to_string(&meta)?)?;
        self.stamps.insert(id, meta.clone());
        Ok(meta)
    }

    pub fn get_stamp(&self, id: Uuid) -> Option<StampMeta> {
        self.stamps.get(&id).map(|m| m.clone())
    }

    pub fn list_stamps(&self) -> Vec<StampMeta> {
        self.stamps.iter().map(|m| m.clone()).collect()
    }

    pub fn delete_stamp(&self, id: Uuid) -> anyhow::Result<()> {
        self.stamps
            .remove(&id)
            .ok_or_else(|| anyhow::anyhow!("stamp not found"))?;
        let _ = std::fs::remove_file(self.stamp_path(id));
        let _ = std::fs::remove_file(self.stamp_meta_path(id));
        Ok(())
    }
}

/// 來源檔 mtime（unix 秒）。拿不到（權限/檔不存在）回 `None`，
/// 與 meta 內 `None` 比對即「無法斷定未變」→ save 需 force。
fn file_mtime(path: &Path) -> Option<u64> {
    std::fs::metadata(path)
        .and_then(|m| m.modified())
        .ok()
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|d| d.as_secs())
}

/// 原子複製：dest 同目錄寫 temp 再 rename。rename 同檔案系統內原子，
/// 斷電最多留下 temp 殘檔，原檔不會半寫。Windows 上 rename 蓋既有檔
/// 若被拒（目標開啟中等），退回先刪再 rename（短暫非原子窗，ADR-004 已記）。
fn atomic_copy(src: &Path, dest: &Path) -> anyhow::Result<()> {
    let dir = dest.parent().unwrap_or_else(|| Path::new("."));
    let tmp = dir.join(format!(
        ".{}.tmp-{}",
        dest.file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| "save".into()),
        Uuid::new_v4().simple()
    ));
    std::fs::copy(src, &tmp)?;
    match std::fs::rename(&tmp, dest) {
        Ok(()) => Ok(()),
        Err(e) if cfg!(windows) => {
            tracing::warn!("atomic rename failed ({e}); falling back to remove+rename");
            std::fs::remove_file(dest)?;
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

#[cfg(test)]
mod tests {
    use super::*;

    fn tmpdir(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("pdfcore-storage-{tag}-{}", Uuid::new_v4()));
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn path_session_open_save_roundtrip() {
        let dir = tmpdir("roundtrip");
        let origin = dir.join("doc.pdf");
        std::fs::write(&origin, b"%PDF-original").unwrap();

        let storage = Storage::new(dir.join("data")).unwrap();
        let meta = storage.open_path(&origin).unwrap();
        assert_eq!(meta.filename, "doc.pdf");
        assert!(meta.origin.is_some());
        assert_eq!(meta.saved_revision, 0);

        // 模擬編輯：工作副本改內容 + bump revision
        std::fs::write(storage.pdf_path(meta.id), b"%PDF-edited").unwrap();
        storage.bump_revision(meta.id).unwrap();

        let saved = storage.save_to_origin(meta.id, false).unwrap();
        assert_eq!(saved.saved_revision, saved.revision);
        assert_eq!(std::fs::read(&origin).unwrap(), b"%PDF-edited");
        // temp 殘檔不得留下
        let leftovers: Vec<_> = std::fs::read_dir(&dir)
            .unwrap()
            .filter_map(|e| e.ok())
            .filter(|e| e.file_name().to_string_lossy().contains(".tmp-"))
            .collect();
        assert!(leftovers.is_empty());
    }

    #[test]
    fn save_detects_external_change() {
        let dir = tmpdir("conflict");
        let origin = dir.join("doc.pdf");
        std::fs::write(&origin, b"%PDF-original").unwrap();

        let storage = Storage::new(dir.join("data")).unwrap();
        let meta = storage.open_path(&origin).unwrap();

        // 外部程式改檔：mtime 改變（filetime 不可用，直接改 meta 模擬時間差）
        {
            let mut m = storage.docs.get_mut(&meta.id).unwrap();
            m.origin_mtime = Some(m.origin_mtime.unwrap() - 10);
        }
        assert!(storage.save_to_origin(meta.id, false).is_err());
        // force 覆寫成功
        assert!(storage.save_to_origin(meta.id, true).is_ok());
    }

    #[test]
    fn save_as_moves_origin() {
        let dir = tmpdir("saveas");
        let origin = dir.join("a.pdf");
        std::fs::write(&origin, b"%PDF-a").unwrap();

        let storage = Storage::new(dir.join("data")).unwrap();
        let meta = storage.open_path(&origin).unwrap();
        let dest = dir.join("b.pdf");
        let saved = storage.save_as(meta.id, &dest).unwrap();
        assert_eq!(saved.filename, "b.pdf");
        assert_eq!(std::fs::read(&dest).unwrap(), b"%PDF-a");
        // 原檔不動
        assert_eq!(std::fs::read(&origin).unwrap(), b"%PDF-a");
    }

    #[test]
    fn upload_session_refuses_save_to_origin() {
        let dir = tmpdir("upload");
        let storage = Storage::new(dir.join("data")).unwrap();
        let meta = storage.save("x.pdf".into(), b"%PDF-x", None).unwrap();
        assert!(storage.save_to_origin(meta.id, false).is_err());
    }
}
