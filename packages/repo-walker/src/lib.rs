//! Walk a directory tree the way a source-code consumer wants to: respect
//! `.gitignore`, skip hidden files, skip known binary extensions, and yield
//! the remaining files through a fallible [`Iterator`].
//!
//! Built on top of [`ignore::WalkBuilder`]; this crate adds a binary-extension
//! filter so callers don't have to maintain their own list, surfaces walk
//! errors as [`Result`] items rather than silently dropping them, and honors
//! the `follow_links` option even for the final "is this a regular file?"
//! check.

use ignore::WalkBuilder;
use std::path::{Path, PathBuf};

pub use ignore::Error as WalkError;

#[derive(Debug, Clone, Copy)]
pub struct WalkOptions {
    /// Honor `.gitignore`, `.git/info/exclude`, the global gitignore file,
    /// and skip hidden entries.
    pub respect_gitignore: bool,
    /// Follow symbolic links during traversal. When false, symlinks are
    /// reported by the walker but skipped by the regular-file filter.
    pub follow_links: bool,
}

impl Default for WalkOptions {
    fn default() -> Self {
        Self {
            respect_gitignore: true,
            follow_links: false,
        }
    }
}

pub struct FileScanner {
    walker: ignore::Walk,
    follow_links: bool,
}

impl FileScanner {
    #[must_use]
    pub fn new(directory: &Path, options: WalkOptions) -> Self {
        // `WalkBuilder` keeps multiple ignore sources on by default: gitignore
        // (incl. global + exclude), generic `.ignore` files, and the hidden
        // filter. `respect_gitignore = false` should silence *all* of those so
        // callers that opt out actually see everything — otherwise stripping
        // git rules but leaving `.ignore` rules behind is confusing.
        let walker = WalkBuilder::new(directory)
            .git_ignore(options.respect_gitignore)
            .git_global(options.respect_gitignore)
            .git_exclude(options.respect_gitignore)
            .ignore(options.respect_gitignore)
            .parents(options.respect_gitignore)
            .hidden(options.respect_gitignore)
            .follow_links(options.follow_links)
            .build();

        Self {
            walker,
            follow_links: options.follow_links,
        }
    }
}

impl Iterator for FileScanner {
    type Item = Result<PathBuf, WalkError>;

    fn next(&mut self) -> Option<Self::Item> {
        loop {
            let entry = match self.walker.next()? {
                Ok(entry) => entry,
                Err(err) => return Some(Err(err)),
            };
            // `DirEntry::file_type` is the type as observed *without*
            // following symlinks. When `follow_links` is off, treat
            // symlinks as not-a-file even if their target is one.
            let Some(file_type) = entry.file_type() else {
                continue;
            };
            let is_regular_file = file_type.is_file()
                || (self.follow_links && file_type.is_symlink() && entry.path().is_file());
            if !is_regular_file {
                continue;
            }
            let ext_ok = entry
                .path()
                .extension()
                .and_then(|ext| ext.to_str())
                .is_none_or(|ext_str| !is_binary_extension(ext_str));
            if ext_ok {
                return Some(Ok(entry.path().to_path_buf()));
            }
        }
    }
}

/// Apply a standalone gitignore matcher to an already-collected path list.
///
/// Useful when paths come from somewhere other than the walker (a file
/// watcher, a diff, a manifest). Only loads the root `.gitignore`; nested
/// `.gitignore` files inside subdirectories are not consulted. For
/// full-tree honor of nested ignores, walk via [`FileScanner`] instead.
pub struct GitignoreFilter {
    root: PathBuf,
    matcher: Option<ignore::gitignore::Gitignore>,
    respect_gitignore: bool,
}

impl GitignoreFilter {
    #[must_use]
    pub fn new(directory: &Path, respect_gitignore: bool) -> Self {
        // Canonicalize the root so the hidden-path strip-prefix check
        // succeeds even when callers pass a relative root but absolute file
        // paths (or vice versa). Fall back to the raw path if the root
        // doesn't exist yet — the matcher will still load (with zero
        // globs) and filter_paths will fall back to a basename check.
        let root = std::fs::canonicalize(directory).unwrap_or_else(|_| directory.to_path_buf());
        let matcher = if respect_gitignore {
            // `GitignoreBuilder::new` only seeds the root, not the file itself.
            // Without an explicit `.add(...)` for the `.gitignore` in the
            // directory, the resulting matcher has zero globs and lets every
            // path through. `add` returns a non-fatal warning when the file
            // is missing; treat that as "no globs to apply" rather than an
            // error.
            let mut builder = ignore::gitignore::GitignoreBuilder::new(&root);
            let _ = builder.add(root.join(".gitignore"));
            builder.build().ok()
        } else {
            None
        };

        Self {
            root,
            matcher,
            respect_gitignore,
        }
    }

    #[must_use]
    pub fn filter_paths(&self, paths: Vec<PathBuf>) -> Vec<PathBuf> {
        let drop_hidden = self.respect_gitignore;
        let matcher = self.matcher.as_ref();
        paths
            .into_iter()
            .filter(|path| {
                if !is_indexable_file(path) {
                    return false;
                }
                // Canonicalize the caller's path so a relative root and an
                // absolute path (or vice versa) line up for both the
                // hidden-prefix check and the gitignore matcher (which is
                // built against the canonicalized root).
                let canonical = std::fs::canonicalize(path).unwrap_or_else(|_| path.clone());
                if drop_hidden && is_hidden_below(&self.root, &canonical) {
                    return false;
                }
                // `matched_path_or_any_parents` walks up the path so a
                // directory-only rule like `target/` excludes
                // `target/debug/foo.rs`. Plain `matched` only checks the
                // path itself, missing descendants.
                matcher.is_none_or(|m| {
                    !m.matched_path_or_any_parents(&canonical, canonical.is_dir())
                        .is_ignore()
                })
            })
            .collect()
    }
}

/// True if any path component beneath `root` starts with `.` (matching the
/// walker's hidden-file rule). Components at or above `root` are ignored so
/// the user's tempdir path or home directory don't accidentally count.
fn is_hidden_below(root: &Path, path: &Path) -> bool {
    let Ok(relative) = path.strip_prefix(root) else {
        // Path isn't under the configured root: fall back to checking the
        // basename only.
        return path
            .file_name()
            .and_then(|n| n.to_str())
            .is_some_and(|s| s.starts_with('.'));
    };
    relative.components().any(|c| {
        c.as_os_str()
            .to_str()
            .is_some_and(|s| s.starts_with('.') && s != "." && s != "..")
    })
}

/// True for regular files (resolved without following symlinks) whose
/// extension is not on the known-binary list. Files without an extension are
/// treated as text.
#[must_use]
pub fn is_indexable_file(path: &Path) -> bool {
    // `Path::is_file` follows symlinks, so a symlink to a regular file would
    // pass. Use `symlink_metadata` so symlinks are excluded; callers that
    // want symlink traversal should pre-resolve and use `Path::is_file`.
    let Ok(metadata) = std::fs::symlink_metadata(path) else {
        return false;
    };
    if !metadata.is_file() {
        return false;
    }

    path.extension()
        .and_then(|ext| ext.to_str())
        .is_none_or(|ext_str| !is_binary_extension(ext_str))
}

#[must_use]
pub fn is_binary_extension(ext: &str) -> bool {
    matches!(
        ext.to_ascii_lowercase().as_str(),
        "exe"
            | "dll"
            | "so"
            | "dylib"
            | "a"
            | "o"
            | "obj"
            | "bin"
            | "png"
            | "jpg"
            | "jpeg"
            | "gif"
            | "bmp"
            | "ico"
            | "svg"
            | "webp"
            | "mp4"
            | "avi"
            | "mov"
            | "wmv"
            | "flv"
            | "mp3"
            | "wav"
            | "flac"
            | "ogg"
            | "zip"
            | "tar"
            | "gz"
            | "bz2"
            | "7z"
            | "rar"
            | "pdf"
            | "doc"
            | "docx"
            | "xls"
            | "xlsx"
            | "ppt"
            | "pptx"
            | "wasm"
            | "class"
            | "jar"
            | "pyc"
            | "pyo"
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn binary_extension_is_case_insensitive() {
        assert!(is_binary_extension("PNG"));
        assert!(is_binary_extension("png"));
        assert!(!is_binary_extension("rs"));
    }

    #[test]
    fn nonexistent_paths_are_not_indexable() {
        assert!(!is_indexable_file(&PathBuf::from("/nonexistent/foo.rs")));
    }

    #[test]
    fn gitignore_filter_loads_dot_gitignore_from_directory() {
        let dir = tempfile::TempDir::new().expect("tempdir");
        std::fs::write(dir.path().join(".gitignore"), "ignored.txt\n").expect("write");
        let keep = dir.path().join("keep.txt");
        let ignored = dir.path().join("ignored.txt");
        std::fs::write(&keep, "k").expect("write keep");
        std::fs::write(&ignored, "i").expect("write ignored");

        let filter = GitignoreFilter::new(dir.path(), true);
        let kept = filter.filter_paths(vec![keep.clone(), ignored]);

        assert_eq!(kept, vec![keep], "ignored.txt should be filtered out");
    }

    #[test]
    fn gitignore_filter_drops_hidden_paths_when_respected() {
        let dir = tempfile::TempDir::new().expect("tempdir");
        let kept = dir.path().join("kept.rs");
        let hidden = dir.path().join(".secret.rs");
        std::fs::write(&kept, "k").expect("write kept");
        std::fs::write(&hidden, "h").expect("write hidden");

        let filter = GitignoreFilter::new(dir.path(), true);
        let result = filter.filter_paths(vec![kept.clone(), hidden]);
        assert_eq!(result, vec![kept]);
    }

    #[test]
    fn gitignore_filter_excludes_ignored_directory_descendants() {
        let dir = tempfile::TempDir::new().expect("tempdir");
        std::fs::write(dir.path().join(".gitignore"), "target/\n").expect("write");
        let target_dir = dir.path().join("target");
        std::fs::create_dir(&target_dir).expect("mkdir target");
        let kept = dir.path().join("keep.rs");
        let ignored = target_dir.join("dropped.rs");
        std::fs::write(&kept, "k").expect("write keep");
        std::fs::write(&ignored, "i").expect("write ignored");

        let filter = GitignoreFilter::new(dir.path(), true);
        let result = filter.filter_paths(vec![kept.clone(), ignored]);

        assert_eq!(
            result,
            vec![kept],
            "files under a directory-rule match should be filtered",
        );
    }

    #[cfg(unix)]
    #[test]
    fn symlink_to_regular_file_is_not_indexable() {
        let dir = tempfile::TempDir::new().expect("tempdir");
        let target = dir.path().join("target.txt");
        let link = dir.path().join("link.txt");
        std::fs::write(&target, "t").expect("write target");
        std::os::unix::fs::symlink(&target, &link).expect("symlink");
        assert!(is_indexable_file(&target));
        assert!(!is_indexable_file(&link), "symlinks should be skipped");
    }

    #[test]
    fn walker_surfaces_missing_root_error() {
        let mut scanner = FileScanner::new(
            &PathBuf::from("/nonexistent/path/that/should/not/exist"),
            WalkOptions::default(),
        );
        let first = scanner.next().expect("walker yields at least one item");
        assert!(first.is_err(), "missing root should surface as an Err");
    }
}
