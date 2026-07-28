//! Enumerating a source tree the way git already sees it.
//!
//! The file set is `git ls-files --cached --others --exclude-standard`: every
//! tracked file plus every untracked file git would offer to add, which is the
//! same thing as "everything except what `.gitignore` covers". Build output
//! (`target/`, `result`, `node_modules/`, `.direnv/`) drops out because the repo
//! already declares it, not because the caller remembered a glob.

use std::ffi::OsStr;
use std::os::unix::ffi::OsStrExt as _;
use std::os::unix::fs::PermissionsExt as _;
use std::path::{Path, PathBuf};
use std::process::Command;

use color_eyre::Result;
use color_eyre::eyre::{WrapErr as _, eyre};
use ignore::WalkBuilder;

/// How a file set was produced.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Origin {
    /// `git ls-files`: tracked plus untracked, minus everything gitignored.
    Git,
    /// A gitignore-aware directory walk, for a tree that git does not know.
    Walk,
}

impl Origin {
    /// A one line description for the run summary.
    #[must_use]
    pub const fn describe(self) -> &'static str {
        match self {
            Self::Git => "git ls-files (tracked + untracked, gitignored excluded)",
            Self::Walk => "directory walk (not a git tree; .gitignore still honoured)",
        }
    }
}

/// One file in the set.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Entry {
    /// Path relative to the sync root.
    pub relative: PathBuf,
    /// Size in bytes. Zero for a symlink, whose target is recreated rather than
    /// copied.
    pub size: u64,
    /// Whether this entry is a symlink.
    pub symlink: bool,
    /// Unix permission bits.
    pub mode: u32,
    /// Modification time, seconds since the epoch, used for the up-to-date
    /// check against the destination.
    pub mtime: i64,
}

/// A file set plus the note about where it came from.
#[derive(Debug)]
pub struct Listing {
    /// How the set was produced.
    pub origin: Origin,
    /// The files, sorted by relative path.
    pub entries: Vec<Entry>,
}

impl Listing {
    /// Total size of every entry.
    #[must_use]
    pub fn bytes(&self) -> u64 {
        self.entries.iter().map(|entry| entry.size).sum()
    }
}

/// Whether git considers `root` part of a working tree.
///
/// Asking git, rather than looking for a `.git` directory, is what makes this
/// work in a linked worktree and inside a submodule. In both of those `.git` is
/// a *file* holding a `gitdir:` line, so a `.git`-is-a-directory test reports
/// "not a git tree" and silently falls back to a plain walk, which then syncs
/// `target/` and every other ignored path.
#[must_use]
pub fn is_git_tree(root: &Path) -> bool {
    git(root, &["rev-parse", "--is-inside-work-tree"])
        .is_ok_and(|stdout| stdout.starts_with(b"true"))
}

/// List the files under `root`.
///
/// # Errors
/// Returns an error if git fails, or if the directory cannot be walked.
pub fn list(root: &Path) -> Result<Listing> {
    let (origin, relatives) = if is_git_tree(root) {
        (Origin::Git, git_paths(root, Path::new(""), 0)?)
    } else {
        (Origin::Walk, walk_paths(root)?)
    };

    let mut entries: Vec<Entry> = Vec::with_capacity(relatives.len());
    for relative in relatives {
        // A tracked-but-deleted path, or a race against an editor, leaves an
        // entry git names and the filesystem does not have. Skipping it is
        // right: there is nothing to send.
        let Ok(metadata) = std::fs::symlink_metadata(root.join(&relative)) else {
            continue;
        };
        if metadata.is_dir() {
            continue;
        }
        let symlink = metadata.is_symlink();
        entries.push(Entry {
            relative,
            size: if symlink { 0 } else { metadata.len() },
            symlink,
            mode: metadata.permissions().mode() & 0o7777,
            mtime: mtime_seconds(&metadata),
        });
    }

    entries.sort_by(|a, b| a.relative.cmp(&b.relative));
    Ok(Listing { origin, entries })
}

/// Seconds since the epoch, or 0 when the platform declines to say.
fn mtime_seconds(metadata: &std::fs::Metadata) -> i64 {
    use std::os::unix::fs::MetadataExt as _;
    metadata.mtime()
}

/// Run git in `dir` and return its stdout.
fn git(dir: &Path, args: &[&str]) -> Result<Vec<u8>> {
    let output = Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(args)
        .output()
        .wrap_err_with(|| format!("could not run git in {}", dir.display()))?;
    if !output.status.success() {
        return Err(eyre!(
            "git {} failed in {}: {}",
            args.join(" "),
            dir.display(),
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }
    Ok(output.stdout)
}

/// Split git's `-z` output into paths.
fn split_nul(bytes: &[u8]) -> Vec<PathBuf> {
    bytes
        .split(|byte| *byte == 0)
        .filter(|record| !record.is_empty())
        .map(|record| PathBuf::from(OsStr::from_bytes(record)))
        .collect()
}

/// Depth limit for submodule recursion, so a cycle in `.gitmodules` cannot spin
/// forever.
const MAX_SUBMODULE_DEPTH: usize = 8;

/// The paths git reports under `dir`, prefixed with `prefix`, recursing into
/// submodules.
fn git_paths(dir: &Path, prefix: &Path, depth: usize) -> Result<Vec<PathBuf>> {
    let listed = git(
        dir,
        &["ls-files", "-z", "--cached", "--others", "--exclude-standard"],
    )?;
    let submodules = gitlinks(dir)?;

    let mut paths: Vec<PathBuf> = split_nul(&listed)
        .into_iter()
        // A submodule appears in ls-files as a single gitlink entry, which is a
        // directory on disk. Drop it here and let the recursion below supply its
        // real contents.
        .filter(|path| !submodules.contains(path))
        .map(|path| prefix.join(path))
        .collect();

    if depth < MAX_SUBMODULE_DEPTH {
        for submodule in submodules {
            let nested = dir.join(&submodule);
            if !nested.is_dir() {
                continue;
            }
            paths.extend(git_paths(&nested, &prefix.join(&submodule), depth + 1)?);
        }
    }

    Ok(paths)
}

/// The submodule paths recorded in `dir`'s index.
fn gitlinks(dir: &Path) -> Result<Vec<PathBuf>> {
    let staged = git(dir, &["ls-files", "-z", "--stage"])?;
    Ok(staged
        .split(|byte| *byte == 0)
        .filter(|record| !record.is_empty())
        .filter_map(|record| {
            // "<mode> <sha> <stage>\t<path>"; mode 160000 is a gitlink.
            let tab = record.iter().position(|byte| *byte == b'\t')?;
            let (meta, path) = record.split_at(tab);
            if !meta.starts_with(b"160000") {
                return None;
            }
            Some(PathBuf::from(OsStr::from_bytes(path.get(1..)?)))
        })
        .collect())
}

/// A gitignore-aware walk, for a tree git does not know about.
fn walk_paths(root: &Path) -> Result<Vec<PathBuf>> {
    let mut paths = Vec::new();
    let walker = WalkBuilder::new(root)
        .hidden(false)
        .git_ignore(true)
        .git_global(true)
        .git_exclude(true)
        .require_git(false)
        .filter_entry(|entry| {
            // Never descend into git's own storage. Matching on the name alone
            // is deliberate: in a linked worktree or a submodule `.git` is a
            // regular file, so a file-type test lets it through and the
            // `gitdir:` pointer is synced to a destination where it means
            // nothing.
            entry.file_name() != ".git"
        })
        .build();

    for result in walker {
        let entry = result.wrap_err_with(|| format!("could not walk {}", root.display()))?;
        let path = entry.path();
        if entry.file_type().is_none_or(|kind| kind.is_dir()) {
            continue;
        }
        let relative = path
            .strip_prefix(root)
            .wrap_err_with(|| format!("{} is not under {}", path.display(), root.display()))?;
        paths.push(relative.to_path_buf());
    }

    Ok(paths)
}
