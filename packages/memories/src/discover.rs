//! Finding `.memories` directories and reading everything in them. No manifest
//! and no index file: the directory listing is the source of truth, so a file
//! added by hand is found on the next run.

use crate::{
    error::{self, Result},
    model::{self, MEMORY_EXTENSION, Memory},
    secret,
};
use snafu::{OptionExt as _, ResultExt as _};
use std::{
    collections::BTreeSet,
    path::{Path, PathBuf},
};

/// Directory name holding a repo's memories.
pub const MEMORIES_DIR_NAME: &str = ".memories";

/// Optional closed set of topics, one per line, read from inside a `.memories`
/// directory. Absent means any topic is allowed.
pub const TOPICS_FILE_NAME: &str = "topics.txt";

/// One place memories live: the directory holding the `.memories` (reported as
/// `root` in JSON) and the `.memories` directory itself.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Root {
    pub root: PathBuf,
    pub memories_dir: PathBuf,
    /// Named by `--dir` rather than discovered. An explicit directory that does
    /// not exist is an error, because the caller asked for it by name; a
    /// discovered one that does not exist is just a repo with no memories.
    pub explicit: bool,
}

impl Root {
    /// Interpret a `--dir` value. A path whose file name is `.memories` is the
    /// memories directory itself; anything else is treated as the directory
    /// that contains one. One rule in both directions, so `--dir .` and
    /// `--dir ./.memories` name the same corpus.
    #[must_use]
    pub fn explicit(path: &Path) -> Self {
        if path
            .file_name()
            .is_some_and(|name| name == MEMORIES_DIR_NAME)
        {
            let root = path.parent().unwrap_or(path).to_path_buf();
            Self {
                root,
                memories_dir: path.to_path_buf(),
                explicit: true,
            }
        } else {
            Self {
                root: path.to_path_buf(),
                memories_dir: path.join(MEMORIES_DIR_NAME),
                explicit: true,
            }
        }
    }

    fn discovered(root: PathBuf) -> Self {
        Self {
            memories_dir: root.join(MEMORIES_DIR_NAME),
            root,
            explicit: false,
        }
    }
}

/// A file in a `.memories` directory that is not a memory: which lint rule it
/// broke, where, and why. Carried alongside the parsed memories so no caller
/// can skip a malformed file without noticing it.
#[derive(Clone, Debug)]
pub struct LoadFailure {
    pub path: PathBuf,
    pub rule: &'static str,
    pub line: Option<usize>,
    pub message: String,
}

/// One directory holding memory files.
///
/// The `.memories` root itself, or one of its immediate subdirectories. What
/// `memory-directory-budget` counts, since the budget is per leaf directory
/// rather than per root.
#[derive(Debug)]
pub struct Leaf {
    pub dir: PathBuf,
    /// Every `*.md` directly in it, parsed or not.
    pub files: usize,
}

/// One `.memories` directory as read from disk.
#[derive(Debug)]
pub struct Scan {
    pub root: Root,
    /// Indices into [`Corpus::memories`], in path order.
    pub memories: Vec<usize>,
    /// Closed topic set from `topics.txt`, or `None` when the file is absent.
    pub topics: Option<BTreeSet<String>>,
    /// The `.memories` directory and each of its immediate subdirectories that
    /// holds memories.
    pub leaves: Vec<Leaf>,
}

/// One line of one file holding a credential, found while reading.
///
/// Collected at load time rather than during linting because it is the one check
/// that must not depend on the file parsing: the first thing anyone does with a
/// parse error is fix the YAML and commit, so a credential found only after a
/// successful parse is found too late.
#[derive(Clone, Debug)]
pub struct SecretFinding {
    pub path: PathBuf,
    pub finding: secret::Finding,
}

/// Every memory found across every root, plus the files that failed to parse and
/// every credential seen on the way past.
#[derive(Debug)]
pub struct Corpus {
    pub scans: Vec<Scan>,
    pub memories: Vec<Memory>,
    pub failures: Vec<LoadFailure>,
    pub secrets: Vec<SecretFinding>,
}

impl Corpus {
    /// Number of memory files read, parsed or not. Reported as `scanned` and
    /// `checked`.
    #[must_use]
    pub const fn scanned(&self) -> usize {
        self.memories.len() + self.failures.len()
    }

    /// First memory with this slug in root order, nearest root first. A slug is
    /// unique within a directory (it is the file stem) but not across roots.
    #[must_use]
    pub fn by_slug(&self, slug: &str) -> Option<&Memory> {
        self.memories.iter().find(|memory| memory.slug == slug)
    }

    /// Every memory with this slug, nearest root first.
    pub fn all_by_slug<'a>(&'a self, slug: &'a str) -> impl Iterator<Item = &'a Memory> {
        self.memories
            .iter()
            .filter(move |memory| memory.slug == slug)
    }

    /// Whether a `related:` or `supersedes:` reference resolves. Resolution is
    /// across every root, not per root: a submodule memory pointing at a
    /// superproject one is a reference that resolves.
    #[must_use]
    pub fn resolves(&self, slug: &str) -> bool {
        self.by_slug(slug).is_some()
    }
}

/// Resolve the roots to read. `dirs` overrides the default set entirely, which
/// is what makes a test or a one-off corpus reproducible: nothing from the
/// ambient repo or home directory leaks in.
///
/// # Errors
///
/// Returns [`crate::Error::NoHomeDir`] when the default set is wanted and no
/// home directory can be resolved.
pub fn resolve_roots(dirs: &[PathBuf], cwd: &Path) -> Result<Vec<Root>> {
    if dirs.is_empty() {
        let home = dirs::home_dir().context(error::NoHomeDirSnafu)?;
        return Ok(default_roots(cwd, &home));
    }
    Ok(dirs
        .iter()
        .map(|dir| Root::explicit(&absolute(dir, cwd)))
        .collect())
}

/// The default set, in precedence order: the git toplevel of `cwd`, then each
/// enclosing git toplevel (submodule before superproject), then `<home>`.
///
/// `home` is a parameter rather than a call to [`dirs::home_dir`] so the
/// resolution order is testable without mutating the environment.
#[must_use]
pub fn default_roots(cwd: &Path, home: &Path) -> Vec<Root> {
    let mut roots: Vec<PathBuf> = Vec::new();
    let mut from = Some(cwd.to_path_buf());
    while let Some(start) = from {
        match git_toplevel(&start) {
            Some(toplevel) => {
                from = toplevel.parent().map(Path::to_path_buf);
                if !roots.contains(&toplevel) {
                    roots.push(toplevel);
                }
            }
            None => break,
        }
    }

    if !roots.contains(&home.to_path_buf()) {
        roots.push(home.to_path_buf());
    }

    roots.into_iter().map(Root::discovered).collect()
}

/// Nearest enclosing directory holding a `.git` entry, starting at `from`. A
/// `.git` file (a worktree or submodule checkout) counts, which is why this
/// tests for existence rather than for a directory.
fn git_toplevel(from: &Path) -> Option<PathBuf> {
    from.ancestors()
        .find(|candidate| candidate.join(".git").exists())
        .map(Path::to_path_buf)
}

fn absolute(path: &Path, cwd: &Path) -> PathBuf {
    if path.is_absolute() {
        path.to_path_buf()
    } else {
        cwd.join(path)
    }
}

/// Read every memory in every root.
///
/// # Errors
///
/// Returns [`crate::Error::MissingMemoriesDir`] when a `--dir` root has no
/// `.memories` directory, and [`crate::Error::ReadDir`] or
/// [`crate::Error::ReadFile`] on an IO failure. A file that reads but does not
/// parse lands in [`Corpus::failures`] instead of failing the load, so one bad
/// file cannot hide the rest of the corpus.
pub fn load(roots: Vec<Root>) -> Result<Corpus> {
    let mut corpus = Corpus {
        scans: Vec::with_capacity(roots.len()),
        memories: Vec::new(),
        failures: Vec::new(),
        secrets: Vec::new(),
    };

    for root in roots {
        if !root.memories_dir.is_dir() {
            if root.explicit {
                return error::MissingMemoriesDirSnafu { path: root.root }.fail();
            }
            continue;
        }

        let listing = memory_paths(&root.memories_dir)?;
        let topics = read_topics(&root.memories_dir.join(TOPICS_FILE_NAME))?;
        let mut indices = Vec::with_capacity(listing.paths.len());

        // A file more than one level deep is not discoverable, and a corpus
        // that silently drops it is the failure this format exists to avoid.
        // Reported under `memory-slug`, the rule that owns "this file's stem is
        // not reachable as a slug".
        for buried in listing.too_deep {
            corpus.failures.push(LoadFailure {
                path: buried,
                rule: "memory-slug",
                line: None,
                message: format!(
                    "more than one level below {dir}, so its stem is not discoverable as a slug",
                    dir = root.memories_dir.display()
                ),
            });
        }

        for path in listing.paths {
            let contents =
                std::fs::read_to_string(&path).context(error::ReadFileSnafu { path: &path })?;

            // Before the parse, so a malformed file cannot hide a credential.
            for finding in secret::scan(&contents) {
                corpus.secrets.push(SecretFinding {
                    path: path.clone(),
                    finding,
                });
            }

            match model::parse_memory(&path, &root.root, &contents) {
                Ok(memory) => {
                    indices.push(corpus.memories.len());
                    corpus.memories.push(memory);
                }
                Err(parse_error) => corpus.failures.push(LoadFailure {
                    path,
                    rule: parse_error.rule,
                    line: parse_error.line,
                    message: parse_error.message,
                }),
            }
        }

        corpus.scans.push(Scan {
            root,
            memories: indices,
            topics,
            leaves: listing.leaves,
        });
    }

    Ok(corpus)
}

/// What one `.memories` directory holds: the memory files, the directories they
/// sit in, and any file buried too deep to be a memory.
struct Listing {
    paths: Vec<PathBuf>,
    leaves: Vec<Leaf>,
    too_deep: Vec<PathBuf>,
}

/// Every `*.md` in one `.memories` directory and in its immediate
/// subdirectories, in path order so output is stable across filesystems.
///
/// One level and no further. A grouping subdirectory keeps a large corpus under
/// the per-directory budget and carries no other meaning: the slug is always the
/// file stem, never the path, so `related:` and `show <slug>` stay
/// location-independent. A file two levels down is collected separately and
/// reported, never dropped.
fn memory_paths(memories_dir: &Path) -> Result<Listing> {
    let mut listing = Listing {
        paths: Vec::new(),
        leaves: Vec::new(),
        too_deep: Vec::new(),
    };

    let top = read_level(memories_dir)?;
    if top.files > 0 {
        listing.leaves.push(Leaf {
            dir: memories_dir.to_path_buf(),
            files: top.files,
        });
    }
    listing.paths.extend(top.paths);

    for directory in top.directories {
        let level = read_level(&directory)?;
        if level.files > 0 {
            listing.leaves.push(Leaf {
                dir: directory,
                files: level.files,
            });
        }
        listing.paths.extend(level.paths);
        // One more level down than the format allows.
        for deeper in level.directories {
            listing.too_deep.extend(read_level(&deeper)?.paths);
        }
    }

    listing.paths.sort();
    listing.too_deep.sort();
    Ok(listing)
}

/// One directory's `*.md` files and its subdirectories.
struct Level {
    paths: Vec<PathBuf>,
    directories: Vec<PathBuf>,
    files: usize,
}

fn read_level(directory: &Path) -> Result<Level> {
    let entries = std::fs::read_dir(directory).context(error::ReadDirSnafu { path: directory })?;
    let mut level = Level {
        paths: Vec::new(),
        directories: Vec::new(),
        files: 0,
    };
    for entry in entries {
        let entry = entry.context(error::ReadDirSnafu { path: directory })?;
        let path = entry.path();
        if path.is_dir() {
            level.directories.push(path);
        } else if path.is_file()
            && path
                .extension()
                .is_some_and(|extension| extension == MEMORY_EXTENSION)
        {
            level.files += 1;
            level.paths.push(path);
        }
    }
    level.paths.sort();
    level.directories.sort();
    Ok(level)
}

/// Read the closed topic set. One topic per line; blank lines and `#` comments
/// are ignored.
fn read_topics(path: &Path) -> Result<Option<BTreeSet<String>>> {
    if !path.is_file() {
        return Ok(None);
    }
    let contents = std::fs::read_to_string(path).context(error::ReadFileSnafu { path })?;
    Ok(Some(
        contents
            .lines()
            .map(str::trim)
            .filter(|line| !line.is_empty() && !line.starts_with('#'))
            .map(ToOwned::to_owned)
            .collect(),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn explicit_dir_accepts_both_the_repo_and_the_memories_directory() {
        let from_repo = Root::explicit(Path::new("/repo"));
        assert_eq!(from_repo.root, Path::new("/repo"));
        assert_eq!(from_repo.memories_dir, Path::new("/repo/.memories"));

        let from_dir = Root::explicit(Path::new("/repo/.memories"));
        assert_eq!(
            from_dir, from_repo,
            "naming the `.memories` directory must resolve the same corpus"
        );
    }

    #[test]
    fn relative_explicit_dirs_are_resolved_against_the_cwd() {
        let roots = resolve_roots(&[PathBuf::from("sub")], Path::new("/repo"))
            .expect("explicit dirs need no home directory");
        assert_eq!(roots.len(), 1);
        assert_eq!(roots[0].memories_dir, Path::new("/repo/sub/.memories"));
    }

    /// A superproject holding a submodule, both git checkouts, with the cwd deep
    /// inside the submodule. `.git` is a directory in one and a file in the
    /// other, which is what a real submodule looks like.
    fn nested_checkouts() -> tempfile::TempDir {
        let dir = tempfile::tempdir().expect("a temp dir");
        let superproject = dir.path().join("super");
        let submodule = superproject.join("index");
        std::fs::create_dir_all(submodule.join("packages/memories")).expect("a source tree");
        std::fs::create_dir_all(superproject.join(".git")).expect("the outer checkout");
        // A submodule checkout carries a `.git` file, not a directory.
        std::fs::write(submodule.join(".git"), "gitdir: ../.git/modules/index\n")
            .expect("the inner checkout");
        dir
    }

    #[test]
    fn default_resolution_walks_submodule_then_superproject_then_home() {
        let tree = nested_checkouts();
        let superproject = tree.path().join("super");
        let submodule = superproject.join("index");
        let home = tree.path().join("home");

        let roots = default_roots(&submodule.join("packages/memories"), &home);
        assert_eq!(
            roots
                .iter()
                .map(|root| root.memories_dir.clone())
                .collect::<Vec<_>>(),
            [
                submodule.join(".memories"),
                superproject.join(".memories"),
                home.join(".memories"),
            ],
            "nearest checkout first, then the superproject, then home"
        );
        assert!(
            roots.iter().all(|root| !root.explicit),
            "a discovered root is not an explicit one, and only explicit roots \
             error when absent"
        );
    }

    #[test]
    fn an_explicit_root_set_suppresses_every_default() {
        let tree = nested_checkouts();
        let submodule = tree.path().join("super/index");
        let elsewhere = tree.path().join("unrelated");

        let named = [elsewhere.clone()];
        let roots =
            resolve_roots(&named, &submodule).expect("explicit dirs need no home directory");
        assert_eq!(
            roots
                .iter()
                .map(|root| root.memories_dir.clone())
                .collect::<Vec<_>>(),
            [elsewhere.join(".memories")],
            "naming a root set means exactly those, with nothing inherited"
        );
        assert!(roots[0].explicit, "and an absent one is then an error");
    }
}
