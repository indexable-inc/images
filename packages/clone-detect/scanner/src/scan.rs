use std::path::{Path, PathBuf};

use ast_merge_ast::tree;
use ast_merge_langs::{Lang, detect};
use clone_hash::{NodeInfo, significant_nodes};
use clone_pragma::scan;
use ignore::WalkBuilder;
use parking_lot::Mutex;
use snafu::ResultExt as _;

use crate::index::Hash;

const ALWAYS_IGNORED_DIRS: &[&str] = &[".git"];

/// cap parallel threads to avoid OOM from concurrent tree-sitter parses
const MAX_THREADS: usize = 8;

#[derive(Debug)]
pub struct File {
    pub path: PathBuf,
    pub language: Lang,
    pub source: String,
    pub nodes: Vec<NodeInfo>,
}

#[derive(Debug, Clone)]
pub struct Config {
    pub min_lines: usize,
    pub min_nodes: usize,
    pub respect_gitignore: bool,
    pub include_hidden: bool,
}

/// Minimum source lines for a fragment to be considered a clone candidate.
const DEFAULT_MIN_LINES: usize = 5;

/// Minimum AST nodes for a fragment to be considered a clone candidate.
const DEFAULT_MIN_NODES: usize = 10;

impl Default for Config {
    fn default() -> Self {
        Self {
            min_lines: DEFAULT_MIN_LINES,
            min_nodes: DEFAULT_MIN_NODES,
            respect_gitignore: true,
            include_hidden: false,
        }
    }
}

#[derive(Debug, snafu::Snafu)]
#[snafu(visibility(pub))]
pub enum Error {
    #[snafu(display("failed to read directory entry"))]
    WalkEntry { source: ignore::Error },

    #[snafu(display("failed to stat {path}"))]
    Stat {
        path: String,
        source: std::io::Error,
    },

    #[snafu(display("failed to read {path}"))]
    ReadFile {
        path: String,
        source: std::io::Error,
    },

    #[snafu(display("failed to parse {path}"))]
    Parse {
        path: String,
        source: ast_merge_ast::Error,
    },
}

pub struct Scanner {
    config: Config,
}

impl Scanner {
    #[must_use]
    pub const fn new(config: Config) -> Self {
        Self { config }
    }

    #[must_use]
    pub fn with_defaults() -> Self {
        Self::new(Config::default())
    }

    /// Scan every file under a directory tree for clone-relevant AST nodes.
    ///
    /// # Errors
    /// Returns an error if the tree cannot be walked or a file cannot be read.
    pub fn directory(&self, path: &Path) -> Result<Output, Error> {
        let results: Mutex<Vec<File>> = Mutex::new(Vec::new());
        let error: Mutex<Option<Error>> = Mutex::new(None);

        let mut builder = WalkBuilder::new(path);
        builder
            .git_ignore(self.config.respect_gitignore)
            .git_global(self.config.respect_gitignore)
            .git_exclude(self.config.respect_gitignore)
            .hidden(!self.config.include_hidden)
            .ignore(self.config.respect_gitignore)
            .threads(MAX_THREADS);
        builder.filter_entry(|entry| {
            let name = entry.file_name();
            name.to_str()
                .is_none_or(|name| !ALWAYS_IGNORED_DIRS.contains(&name))
        });

        builder.build_parallel().run(|| {
            let results = &results;
            let error = &error;
            Box::new(move |entry| {
                if error.lock().is_some() {
                    return ignore::WalkState::Quit;
                }

                let entry = match entry {
                    Ok(e) => e,
                    Err(e) => {
                        *error.lock() = Some(Error::WalkEntry { source: e });
                        return ignore::WalkState::Quit;
                    }
                };

                if entry.file_type().is_some_and(|ft| ft.is_dir()) {
                    return ignore::WalkState::Continue;
                }

                match self.file(entry.path()) {
                    Ok(Some(scanned)) => {
                        results.lock().push(scanned);
                    }
                    Ok(None) => {}
                    Err(e) => {
                        *error.lock() = Some(e);
                        return ignore::WalkState::Quit;
                    }
                }

                ignore::WalkState::Continue
            })
        });

        if let Some(e) = error.into_inner() {
            return Err(e);
        }

        let mut files = results.into_inner();
        files.sort_by(|a, b| a.path.cmp(&b.path));
        let mut index = Hash::new();
        for (file_id, file) in files.iter().enumerate() {
            for (node_idx, node) in file.nodes.iter().enumerate() {
                index.add(&crate::index::Entry { file_id, node_idx }, node);
            }
        }

        Ok(Output { files, index })
    }

    /// Scan a single file for clone-relevant AST nodes.
    ///
    /// # Errors
    /// Returns an error if the file cannot be stat-ed, read, or parsed.
    pub fn file(&self, path: &Path) -> Result<Option<File>, Error> {
        let path_str = path.display().to_string();
        let metadata = std::fs::symlink_metadata(path).context(StatSnafu { path: &path_str })?;
        if metadata.file_type().is_symlink() || !metadata.is_file() {
            return Ok(None);
        }

        let Some(language) = detect(path) else {
            return Ok(None);
        };

        let bytes = match std::fs::read(path) {
            Ok(bytes) => bytes,
            Err(err) => {
                let is_broken_symlink = err.kind() == std::io::ErrorKind::NotFound
                    && std::fs::symlink_metadata(path).is_ok_and(|m| m.file_type().is_symlink());
                if is_broken_symlink {
                    tracing::warn!(
                        path = %path.display(),
                        "skipping broken symlink during clone scan",
                    );
                    return Ok(None);
                }
                return Err(err).context(ReadFileSnafu { path: path_str });
            }
        };

        let Ok(source) = String::from_utf8(bytes) else {
            return Ok(None);
        };

        let tree =
            tree(&source, &language.to_tree_sitter()).context(ParseSnafu { path: path_str })?;

        let pragma_info = scan(&tree.tree);

        if pragma_info.ignore_file {
            return Ok(None);
        }

        let nodes = significant_nodes(&tree.tree, self.config.min_lines, self.config.min_nodes)
            .into_iter()
            .filter(|node| !pragma_info.is_ignored(&node.byte_range))
            .collect();

        Ok(Some(File {
            path: path.to_path_buf(),
            language,
            source,
            nodes,
        }))
    }
}

#[derive(Debug)]
pub struct Output {
    pub files: Vec<File>,
    pub index: Hash,
}

impl Output {
    #[must_use]
    pub fn total_nodes(&self) -> usize {
        self.files.iter().map(|f| f.nodes.len()).sum()
    }

    #[must_use]
    pub const fn total_files(&self) -> usize {
        self.files.len()
    }
}
