use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs;
use std::path::Path;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FileState {
    pub size: u64,
    pub mtime_ns: u128,
    pub hash: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TargetState {
    pub fingerprint: String,
    pub files: HashMap<String, FileState>,
}

#[derive(Debug, Default, Serialize, Deserialize)]
pub struct Cache {
    pub targets: HashMap<String, TargetState>,
}

impl Cache {
    pub fn load(path: &Path) -> Result<Self> {
        if !path.exists() {
            return Ok(Self::default());
        }

        let source = fs::read_to_string(path)
            .with_context(|| format!("could not read cache {}", path.display()))?;

        let cache = serde_json::from_str(&source)
            .with_context(|| format!("invalid cache {}", path.display()))?;

        Ok(cache)
    }

    pub fn save(&self, path: &Path) -> Result<()> {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }

        let source = serde_json::to_string_pretty(self)?;
        fs::write(path, source)
            .with_context(|| format!("could not write cache {}", path.display()))?;

        Ok(())
    }
}
