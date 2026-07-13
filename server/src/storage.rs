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

    pub fn save(&self, filename: String, bytes: &[u8]) -> anyhow::Result<DocMeta> {
        let id = Uuid::new_v4();
        std::fs::write(self.pdf_path(id), bytes)?;
        let meta = DocMeta {
            id,
            filename,
            size: bytes.len() as u64,
            revision: 0,
        };
        std::fs::write(self.meta_path(id), serde_json::to_string(&meta)?)?;
        self.docs.insert(id, meta.clone());
        Ok(meta)
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
