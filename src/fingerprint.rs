use anyhow::{Context, Result, bail};
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::UNIX_EPOCH;
use walkdir::WalkDir;

use crate::cache::FileState;
use crate::model::Target;

pub struct FingerprintResult {
    pub fingerprint: String,
    pub files: HashMap<String, FileState>,
}

pub fn calculate(
    root: &Path,
    target: &Target,
    previous: Option<&HashMap<String, FileState>>,
    dependency_fingerprints: &[String],
) -> Result<FingerprintResult> {
    let mut paths = Vec::new();

    for watch in &target.watches {
        expand_watch(root, watch, &mut paths)?;
    }

    paths.sort();
    paths.dedup();

    let mut files = HashMap::new();
    let mut hasher = Sha256::new();

    for path in paths {
        let relative = path
            .strip_prefix(root)
            .unwrap_or(&path)
            .to_string_lossy()
            .to_string();

        let metadata = fs::symlink_metadata(&path)
            .with_context(|| format!("could not stat {}", path.display()))?;

        if metadata.is_dir() {
            bail!("watch path resolved to directory: {}", path.display());
        }

        let size = metadata.len();

        let mtime = metadata
            .modified()?
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos();

        let old = previous.and_then(|files| files.get(&relative));

        let hash = if let Some(old) = old {
            if old.size == size && old.mtime_ns == mtime {
                old.hash.clone()
            } else {
                hash_file(&path)?
            }
        } else {
            hash_file(&path)?
        };

        hasher.update(relative.as_bytes());
        hasher.update([0]);
        hasher.update(hash.as_bytes());
        hasher.update([0]);

        files.insert(
            relative,
            FileState {
                size,
                mtime_ns: mtime,
                hash,
            },
        );
    }

    for dependency in dependency_fingerprints {
        hasher.update(dependency.as_bytes());
        hasher.update([0]);
    }

    for line in &target.recipe {
        hasher.update(line.as_bytes());
        hasher.update([b'\n']);
    }

    for output in &target.outputs {
        hasher.update(output.to_string_lossy().as_bytes());
        hasher.update([0]);
    }

    Ok(FingerprintResult {
        fingerprint: format!("{:x}", hasher.finalize()),
        files,
    })
}

fn hash_file(path: &Path) -> Result<String> {
    let bytes = fs::read(path).with_context(|| format!("could not read {}", path.display()))?;

    let mut hasher = Sha256::new();
    hasher.update(bytes);

    Ok(format!("{:x}", hasher.finalize()))
}

fn expand_watch(root: &Path, pattern: &str, output: &mut Vec<PathBuf>) -> Result<()> {
    let full = root.join(pattern);

    if !pattern.contains('*') {
        if !full.exists() {
            bail!("watch path does not exist: {}", full.display());
        }

        if full.is_dir() {
            bail!(
                "directory cannot be watched directly; use /** or /*: {}",
                full.display()
            );
        }

        output.push(full);
        return Ok(());
    }

    if pattern.ends_with("/**") {
        let base = pattern.trim_end_matches("/**");
        let base = root.join(base);

        if !base.is_dir() {
            bail!("watch directory does not exist: {}", base.display());
        }

        for entry in WalkDir::new(&base).follow_links(false) {
            let entry = entry?;

            if entry.file_type().is_file() || entry.file_type().is_symlink() {
                output.push(entry.path().to_path_buf());
            }
        }

        return Ok(());
    }

    if pattern.ends_with("/*") {
        let base = pattern.trim_end_matches("/*");
        let base = root.join(base);

        if !base.is_dir() {
            bail!("watch directory does not exist: {}", base.display());
        }

        for entry in fs::read_dir(&base)? {
            let entry = entry?;
            let ty = entry.file_type()?;

            if ty.is_file() || ty.is_symlink() {
                output.push(entry.path());
            }
        }

        return Ok(());
    }

    bail!("unsupported watch pattern: {}", pattern)
}
