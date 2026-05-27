//! Walk a directory tree the way a source-code consumer wants to: respect
//! `.gitignore`, skip hidden files, skip known binary extensions, and yield
//! the remaining paths through a plain [`Iterator`].
//!
//! Built on top of [`ignore::WalkBuilder`]; this crate adds a binary-extension
//! filter so callers don't have to maintain their own list.

use ignore::WalkBuilder;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Copy)]
pub struct WalkOptions {
    /// Honor `.gitignore`, `.git/info/exclude`, the global gitignore file,
    /// and skip hidden entries.
    pub respect_gitignore: bool,
    /// Follow symbolic links during traversal.
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
}

impl FileScanner {
    #[must_use]
    pub fn new(directory: &Path, options: WalkOptions) -> Self {
        let walker = WalkBuilder::new(directory)
            .git_ignore(options.respect_gitignore)
            .git_global(options.respect_gitignore)
            .git_exclude(options.respect_gitignore)
            .hidden(options.respect_gitignore)
            .follow_links(options.follow_links)
            .build();

        Self { walker }
    }
}

impl Iterator for FileScanner {
    type Item = PathBuf;

    fn next(&mut self) -> Option<Self::Item> {
        (&mut self.walker)
            .flatten()
            .find(|entry| is_indexable_file(entry.path()))
            .map(|entry| entry.path().to_path_buf())
    }
}

/// Apply a standalone gitignore matcher to an already-collected path list.
/// Useful when paths come from somewhere other than the walker (a file
/// watcher, a diff, a manifest).
pub struct GitignoreFilter {
    matcher: Option<ignore::gitignore::Gitignore>,
}

impl GitignoreFilter {
    #[must_use]
    pub fn new(directory: &Path, respect_gitignore: bool) -> Self {
        let matcher = if respect_gitignore {
            // `GitignoreBuilder::new` only seeds the root, not the file itself.
            // Without an explicit `.add(...)` for the `.gitignore` in the
            // directory, the resulting matcher has zero globs and lets every
            // path through. `add` returns a non-fatal warning when the file
            // is missing; treat that as "no globs to apply" rather than an
            // error.
            let mut builder = ignore::gitignore::GitignoreBuilder::new(directory);
            let _ = builder.add(directory.join(".gitignore"));
            builder.build().ok()
        } else {
            None
        };

        Self { matcher }
    }

    #[must_use]
    pub fn filter_paths(&self, paths: Vec<PathBuf>) -> Vec<PathBuf> {
        let Some(matcher) = self.matcher.as_ref() else {
            return paths.into_iter().filter(|p| is_indexable_file(p)).collect();
        };

        paths
            .into_iter()
            .filter(|path| {
                is_indexable_file(path) && !matcher.matched(path, path.is_dir()).is_ignore()
            })
            .collect()
    }
}

/// True for regular files whose extension is not on the known-binary list.
/// Files without an extension are treated as text.
#[must_use]
pub fn is_indexable_file(path: &Path) -> bool {
    if !path.is_file() {
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
}
