use std::path::{Path, PathBuf};

use dashmap::DashMap;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

#[derive(Clone, Serialize, Deserialize)]
pub struct DocMeta {
    pub id: Uuid,
    pub filename: String,
    pub size: u64,
}

pub struct Storage {
    root: PathBuf,
    docs: DashMap<Uuid, DocMeta>,
}

impl Storage {
    pub fn new(root: impl AsRef<Path>) -> anyhow::Result<Self> {
        let root = root.as_ref().to_path_buf();
        std::fs::create_dir_all(&root)?;
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
        Ok(Self { root, docs })
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
        };
        std::fs::write(self.meta_path(id), serde_json::to_string(&meta)?)?;
        self.docs.insert(id, meta.clone());
        Ok(meta)
    }

    pub fn get(&self, id: Uuid) -> Option<DocMeta> {
        self.docs.get(&id).map(|m| m.clone())
    }

    pub fn list(&self) -> Vec<DocMeta> {
        self.docs.iter().map(|m| m.clone()).collect()
    }
}
