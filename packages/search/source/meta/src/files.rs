//! Shared filesystem traversal for local source adapters.

use std::io;
use std::path::{Path, PathBuf};

#[derive(Debug)]
pub struct WalkError {
    pub path: PathBuf,
    pub source: io::Error,
}

/// Recursively collect matching regular files without following symlinks below
/// the explicitly named root. A missing root is an empty corpus.
pub fn collect_no_follow(
    root: &Path,
    out: &mut Vec<PathBuf>,
    matches: impl Copy + Fn(&Path) -> bool,
) -> Result<(), WalkError> {
    let entries = match std::fs::read_dir(root) {
        Ok(entries) => entries,
        Err(source) if source.kind() == io::ErrorKind::NotFound => return Ok(()),
        Err(source) => {
            return Err(WalkError {
                path: root.to_path_buf(),
                source,
            });
        }
    };
    for entry in entries {
        let entry = entry.map_err(|source| WalkError {
            path: root.to_path_buf(),
            source,
        })?;
        let file_type = entry.file_type().map_err(|source| WalkError {
            path: root.to_path_buf(),
            source,
        })?;
        if file_type.is_symlink() {
            continue;
        }
        let path = entry.path();
        if file_type.is_dir() {
            collect_no_follow(&path, out, matches)?;
        } else if file_type.is_file() && matches(&path) {
            out.push(path);
        }
    }
    Ok(())
}

/// Collect JSONL files and adapt traversal failures to the caller's error type.
pub fn collect_jsonl_no_follow<E>(
    root: &Path,
    out: &mut Vec<PathBuf>,
    map_error: impl FnOnce(WalkError) -> E,
) -> Result<(), E> {
    collect_no_follow(root, out, |path| {
        path.extension().is_some_and(|extension| extension == "jsonl")
    })
    .map_err(map_error)
}
