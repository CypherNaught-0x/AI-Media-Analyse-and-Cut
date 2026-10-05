//! Where derived media (analysis audio, preview audio, silence-trimmed audio,
//! chunks) lives: one folder per source file in the app's cache directory,
//! instead of next to the user's media.
//!
//! The folder is keyed by the source's path, size and modification time, so
//! an edited or replaced source never picks up stale derived files.

use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};
use tauri::Manager;

/// Remove cache entries that haven't been used for this long...
const MAX_AGE: Duration = Duration::from_secs(30 * 24 * 60 * 60);
/// ...and then the least recently used ones until the cache fits this budget.
const MAX_BYTES: u64 = 10 * 1024 * 1024 * 1024;

/// `<app cache>/derived-media`.
pub(crate) fn cache_root(app: &tauri::AppHandle) -> Result<PathBuf, String> {
    app.path()
        .app_cache_dir()
        .map(|dir| dir.join("derived-media"))
        .map_err(|error| format!("Failed to resolve the app cache directory: {error}"))
}

/// The cache folder for `source` (created if needed). Using it marks it as
/// recently used for [`prune`].
pub(crate) fn source_dir(root: &Path, source: &Path) -> Result<PathBuf, String> {
    let dir = root.join(source_key(source)?);
    std::fs::create_dir_all(&dir)
        .map_err(|error| format!("Failed to create cache folder '{}': {error}", dir.display()))?;
    touch(&dir);
    Ok(dir)
}

/// The cache folder for `source` if it already exists.
pub(crate) fn existing_source_dir(root: &Path, source: &Path) -> Option<PathBuf> {
    let dir = root.join(source_key(source).ok()?);
    dir.is_dir().then_some(dir)
}

/// A short key for `source` that changes when the file does.
pub(crate) fn source_key(source: &Path) -> Result<String, String> {
    let canonical = std::fs::canonicalize(source)
        .map_err(|error| format!("Failed to resolve '{}': {error}", source.display()))?;
    let metadata = std::fs::metadata(&canonical)
        .map_err(|error| format!("Failed to read '{}': {error}", source.display()))?;
    let modified = metadata
        .modified()
        .ok()
        .and_then(|time| time.duration_since(SystemTime::UNIX_EPOCH).ok())
        .map_or(0, |since| since.as_nanos());

    let mut hasher = Sha256::new();
    hasher.update(canonical.to_string_lossy().as_bytes());
    hasher.update(metadata.len().to_le_bytes());
    hasher.update(modified.to_le_bytes());
    Ok(format!("{:x}", hasher.finalize())[..20].to_string())
}

fn touch(dir: &Path) {
    let _ = std::fs::File::open(dir).and_then(|file| file.set_modified(SystemTime::now()));
}

/// Delete entries unused for [`MAX_AGE`], then the least recently used ones
/// until the total is within [`MAX_BYTES`]. Best effort: errors are skipped.
pub(crate) fn prune(root: &Path) {
    prune_with(root, MAX_AGE, MAX_BYTES, SystemTime::now());
}

fn prune_with(root: &Path, max_age: Duration, max_bytes: u64, now: SystemTime) {
    let Ok(entries) = std::fs::read_dir(root) else {
        return;
    };
    let mut kept: Vec<(SystemTime, u64, PathBuf)> = Vec::new();
    for entry in entries.flatten() {
        let path = entry.path();
        if !path.is_dir() {
            continue;
        }
        let used = entry
            .metadata()
            .and_then(|meta| meta.modified())
            .unwrap_or(SystemTime::UNIX_EPOCH);
        if now.duration_since(used).unwrap_or_default() > max_age {
            remove(&path);
        } else {
            kept.push((used, dir_size(&path), path));
        }
    }

    let mut total: u64 = kept.iter().map(|(_, size, _)| size).sum();
    kept.sort_by_key(|(used, _, _)| *used);
    for (_, size, path) in kept {
        if total <= max_bytes {
            break;
        }
        remove(&path);
        total = total.saturating_sub(size);
    }
}

fn remove(path: &Path) {
    if let Err(error) = std::fs::remove_dir_all(path) {
        log::warn!("Failed to prune cache entry '{}': {error}", path.display());
    }
}

fn dir_size(dir: &Path) -> u64 {
    std::fs::read_dir(dir)
        .into_iter()
        .flatten()
        .flatten()
        .map(|entry| {
            let path = entry.path();
            if path.is_dir() {
                dir_size(&path)
            } else {
                entry.metadata().map_or(0, |meta| meta.len())
            }
        })
        .sum()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_changed_source_gets_a_different_folder() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("cache");
        let source = dir.path().join("talk.mp4");
        std::fs::write(&source, b"original").unwrap();

        let first = source_dir(&root, &source).unwrap();
        assert_eq!(source_dir(&root, &source).unwrap(), first);
        assert_eq!(existing_source_dir(&root, &source), Some(first.clone()));

        std::fs::write(&source, b"edited, and longer").unwrap();
        assert_ne!(source_dir(&root, &source).unwrap(), first);
    }

    #[test]
    fn missing_sources_have_no_folder() {
        let dir = tempfile::tempdir().unwrap();
        assert!(source_dir(dir.path(), &dir.path().join("missing.mp4")).is_err());
        assert_eq!(
            existing_source_dir(dir.path(), &dir.path().join("missing.mp4")),
            None
        );
    }

    fn entry(root: &Path, name: &str, bytes: usize, age: Duration) -> PathBuf {
        let path = root.join(name);
        std::fs::create_dir_all(&path).unwrap();
        std::fs::write(path.join("analysis.ogg"), vec![0u8; bytes]).unwrap();
        std::fs::File::open(&path)
            .unwrap()
            .set_modified(SystemTime::now() - age)
            .unwrap();
        path
    }

    #[test]
    fn pruning_drops_old_entries_then_the_least_recently_used() {
        let dir = tempfile::tempdir().unwrap();
        let day = Duration::from_secs(24 * 60 * 60);
        let stale = entry(dir.path(), "stale", 10, 40 * day);
        let oldest = entry(dir.path(), "oldest", 600, 3 * day);
        let recent = entry(dir.path(), "recent", 600, day);
        let newest = entry(dir.path(), "newest", 10, Duration::ZERO);

        prune_with(dir.path(), 30 * day, 1000, SystemTime::now());

        assert!(!stale.exists(), "unused for longer than the max age");
        assert!(!oldest.exists(), "least recently used, over budget");
        assert!(recent.exists() && newest.exists());
    }
}
