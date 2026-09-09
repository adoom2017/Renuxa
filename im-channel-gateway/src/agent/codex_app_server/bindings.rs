use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use serde::{Deserialize, Serialize};

use crate::error::{GatewayError, Result};

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ThreadBinding {
    pub thread_id: String,
    #[serde(default)]
    pub cwd: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
struct BindingFile {
    #[serde(default)]
    sessions: HashMap<String, ThreadBinding>,
}

pub struct BindingStore {
    path: PathBuf,
    inner: Mutex<BindingFile>,
}

impl BindingStore {
    pub fn load(data_dir: &Path) -> Result<Self> {
        let path = data_dir.join("codex_thread_bindings.json");
        let file = if path.exists() {
            let raw = std::fs::read_to_string(&path)
                .map_err(|e| GatewayError::Other(format!("read bindings: {e}")))?;
            serde_json::from_str(&raw).unwrap_or_default()
        } else {
            BindingFile::default()
        };
        Ok(Self {
            path,
            inner: Mutex::new(file),
        })
    }

    pub fn get(&self, im_session_id: &str) -> Option<ThreadBinding> {
        self.inner
            .lock()
            .ok()
            .and_then(|g| g.sessions.get(im_session_id).cloned())
    }

    pub fn set(&self, im_session_id: &str, binding: ThreadBinding) -> Result<()> {
        {
            let mut guard = self
                .inner
                .lock()
                .map_err(|_| GatewayError::Other("bindings lock poisoned".into()))?;
            guard.sessions.insert(im_session_id.to_string(), binding);
            self.persist(&guard)?;
        }
        Ok(())
    }

    pub fn clear(&self, im_session_id: &str) -> Result<()> {
        {
            let mut guard = self
                .inner
                .lock()
                .map_err(|_| GatewayError::Other("bindings lock poisoned".into()))?;
            guard.sessions.remove(im_session_id);
            self.persist(&guard)?;
        }
        Ok(())
    }

    fn persist(&self, file: &BindingFile) -> Result<()> {
        if let Some(parent) = self.path.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|e| GatewayError::Other(format!("create bindings dir: {e}")))?;
        }
        let raw = serde_json::to_string_pretty(file)
            .map_err(|e| GatewayError::Other(format!("serialize bindings: {e}")))?;
        std::fs::write(&self.path, raw)
            .map_err(|e| GatewayError::Other(format!("write bindings: {e}")))?;
        Ok(())
    }
}
