//! Checks on paths the frontend passes to file-system commands.
//!
//! These commands run with the user's full permissions, so each one only
//! accepts the kind of path it exists for. If script ever got injected into
//! the webview, it couldn't turn them into "read/write any file" or "launch
//! any program".

use std::path::{Path, PathBuf};

/// The file must have one of `allowed` extensions (case-insensitive).
pub(crate) fn require_extension(
    path: &Path,
    allowed: &[&str],
    purpose: &str,
) -> Result<(), String> {
    let extension = path
        .extension()
        .and_then(|ext| ext.to_str())
        .map(str::to_ascii_lowercase)
        .unwrap_or_default();
    if allowed.contains(&extension.as_str()) {
        Ok(())
    } else {
        Err(format!(
            "Refusing to {purpose} '{}': only .{} files are allowed",
            path.display(),
            allowed.join(", .")
        ))
    }
}

/// The path must resolve to something inside `root` (after following
/// symlinks and `..`). Returns the resolved path.
pub(crate) fn require_within(path: &Path, root: &Path, purpose: &str) -> Result<PathBuf, String> {
    let resolved = std::fs::canonicalize(path)
        .map_err(|error| format!("Failed to {purpose} '{}': {error}", path.display()))?;
    let root =
        std::fs::canonicalize(root).map_err(|error| format!("Failed to {purpose}: {error}"))?;
    if resolved.starts_with(&root) {
        Ok(resolved)
    } else {
        Err(format!(
            "Refusing to {purpose} '{}': it is outside the app's media cache",
            path.display()
        ))
    }
}

/// The path must be an existing directory (opening a file instead would
/// launch it, e.g. an .app or .exe).
pub(crate) fn require_directory(path: &Path) -> Result<(), String> {
    if path.is_dir() {
        Ok(())
    } else {
        Err(format!("'{}' is not a folder", path.display()))
    }
}

/// True if `path` names a Python interpreter (`python`, `python3`,
/// `python3.12`, `python.exe`, or the Windows `py` launcher), so a configured
/// interpreter override can't point at an arbitrary program.
pub(crate) fn looks_like_python(path: &str) -> bool {
    // Split on both separators: the setting may hold a Windows path.
    let name = path
        .trim()
        .rsplit(['/', '\\'])
        .next()
        .unwrap_or_default()
        .to_ascii_lowercase();
    let name = name.strip_suffix(".exe").unwrap_or(&name);
    if name == "py" {
        return true;
    }
    let Some(version) = name.strip_prefix("python") else {
        return false;
    };
    version.is_empty()
        || version
            .split('.')
            .all(|part| !part.is_empty() && part.chars().all(|c| c.is_ascii_digit()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extensions_are_checked_case_insensitively() {
        assert!(require_extension(Path::new("/a/talk.transcript.json"), &["json"], "read").is_ok());
        assert!(require_extension(Path::new("/a/talk.SRT"), &["srt", "vtt"], "write").is_ok());
        let error = require_extension(Path::new("/a/.zshrc"), &["json"], "write").unwrap_err();
        assert!(error.contains("only .json"), "{error}");
        assert!(require_extension(Path::new("/a/run.sh"), &["srt", "vtt"], "write").is_err());
    }

    #[test]
    fn paths_must_stay_inside_the_root() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("cache");
        std::fs::create_dir_all(root.join("entry")).unwrap();
        std::fs::write(root.join("entry/analysis.ogg"), b"x").unwrap();
        std::fs::write(dir.path().join("secret.txt"), b"x").unwrap();

        assert!(require_within(&root.join("entry/analysis.ogg"), &root, "read").is_ok());
        assert!(require_within(&root.join("entry/../../secret.txt"), &root, "read").is_err());
        assert!(require_within(&dir.path().join("secret.txt"), &root, "read").is_err());
    }

    #[test]
    fn only_directories_can_be_opened() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("tool.app");
        std::fs::write(&file, b"x").unwrap();
        assert!(require_directory(dir.path()).is_ok());
        assert!(require_directory(&file).is_err());
    }

    #[test]
    fn python_interpreter_names() {
        for ok in [
            "python",
            "python3",
            "/opt/homebrew/bin/python3.12",
            "C:\\Python312\\python.exe",
            "py",
            "/env/bin/python3",
        ] {
            assert!(looks_like_python(ok), "{ok}");
        }
        for bad in ["/bin/sh", "pythonista", "python3.12.evil", "calc.exe", ""] {
            assert!(!looks_like_python(bad), "{bad}");
        }
    }
}
