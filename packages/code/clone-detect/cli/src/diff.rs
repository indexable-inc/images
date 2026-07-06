//! Git-backed changed-line discovery for the diff gate.
//!
//! The diff gate asks: of the lines this change touched, how many landed on
//! duplicated code? "Touched" means added or modified lines on the new side of
//! `git diff <merge-base(base, HEAD)>`, taken against the working tree so
//! uncommitted edits count. This module owns the process concerns (invoking
//! `git`, resolving the merge base) and the pure unified-diff hunk parser; the
//! gate math lives in [`crate::gate`].

use std::{
    collections::{BTreeMap, BTreeSet},
    path::{Path, PathBuf},
    process::Command,
};

use snafu::{OptionExt as _, ResultExt as _, ensure};

/// Added/modified line numbers per file, 1-indexed on the new (working-tree)
/// side. A file with only deletions contributes no lines.
///
/// Keys from [`changed_lines`] are absolute paths (repo root joined with git's
/// repo-relative path, canonicalized where possible) so they can be matched
/// against clone-fragment paths regardless of how the scan target was spelled.
/// [`parse_unified_diff`] on its own returns git's repo-relative paths; the
/// absolutization happens in [`changed_lines`].
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ChangedLines(pub BTreeMap<PathBuf, BTreeSet<usize>>);

/// The resolved diff for the gate: the merge-base commit it was taken against
/// and the changed lines on the working-tree side.
pub struct RepoDiff {
    pub base_sha: String,
    pub changed: ChangedLines,
}

#[derive(Debug, snafu::Snafu)]
pub enum DiffError {
    #[snafu(display("failed to run `git {args}`: is git installed and on PATH?"))]
    Spawn {
        args: String,
        source: std::io::Error,
    },

    #[snafu(display(
        "`git {args}` failed (exit {code}): {stderr}",
        code = code.map_or_else(|| "signal".to_owned(), |c| c.to_string()),
    ))]
    Command {
        args: String,
        code: Option<i32>,
        stderr: String,
    },

    #[snafu(display("`git {args}` printed non-UTF-8 output"))]
    NonUtf8 {
        args: String,
        source: std::string::FromUtf8Error,
    },

    #[snafu(display(
        "could not find a merge base between {base:?} and HEAD; \
         is {base:?} a known revision? (fetch it, or pass a different --diff base)"
    ))]
    NoMergeBase { base: String },

    #[snafu(display("malformed diff hunk header: {line:?}"))]
    BadHunkHeader { line: String },
}

/// A `git` invocation neutralized against the caller's environment: user/system
/// config, pager, and external diff drivers are all disabled so output is a
/// stable, machine-parseable unified diff regardless of the caller's
/// `~/.gitconfig` (a columnar `diff.external`, a pager, or color codes would all
/// corrupt the parse). `dir` anchors the command in the scanned repository, not
/// the process's own working directory.
fn git_command(dir: &Path) -> Command {
    let mut command = Command::new("git");
    command
        .current_dir(dir)
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_SYSTEM", "/dev/null")
        .env("GIT_PAGER", "cat")
        .env("GIT_OPTIONAL_LOCKS", "0");
    command
}

/// Run `git` in `dir`, returning stdout on a zero exit or a precise error
/// otherwise (never a silent fallback: the diff gate must fail loudly when git
/// cannot answer).
fn git(dir: &Path, args: &[&str]) -> Result<String, DiffError> {
    let display = args.join(" ");
    let output = git_command(dir)
        .args(args)
        .output()
        .context(SpawnSnafu { args: &display })?;

    ensure!(
        output.status.success(),
        CommandSnafu {
            args: &display,
            code: output.status.code(),
            stderr: String::from_utf8_lossy(&output.stderr).trim().to_owned(),
        }
    );

    String::from_utf8(output.stdout).context(NonUtf8Snafu { args: &display })
}

/// Resolve the merge base of `base` and `HEAD` in the repository at `dir`. Fails
/// loudly when `base` is unknown or the two revisions share no history.
pub fn merge_base(dir: &Path, base: &str) -> Result<String, DiffError> {
    // `git merge-base` exits 1 with empty output when there is no common
    // ancestor and nonzero with a message when `base` is not a valid revision;
    // distinguish the "no ancestor" case for a clearer error.
    let display = format!("merge-base {base} HEAD");
    let output = git_command(dir)
        .args(["merge-base", base, "HEAD"])
        .output()
        .context(SpawnSnafu { args: &display })?;

    if output.status.success() {
        let sha = String::from_utf8(output.stdout)
            .context(NonUtf8Snafu { args: &display })?
            .trim()
            .to_owned();
        // Success with empty output means the revisions share no common
        // ancestor; treat it as the same failure as an unknown base.
        ensure!(!sha.is_empty(), NoMergeBaseSnafu { base });
        return Ok(sha);
    }

    // A nonzero exit with no diagnostic is git's "not a valid object" for the
    // base rev; surface the actionable message rather than an opaque exit code.
    let stderr = String::from_utf8_lossy(&output.stderr);
    ensure!(!stderr.trim().is_empty(), NoMergeBaseSnafu { base });
    CommandSnafu {
        args: &display,
        code: output.status.code(),
        stderr: stderr.trim().to_owned(),
    }
    .fail()
}

/// Changed lines relative to the merge base of `base` and `HEAD`, resolved in
/// the repository at `dir` and including uncommitted working-tree edits. Diffing
/// `<merge-base>` (a single tree argument, no `--cached`) against the working
/// tree is what folds committed and uncommitted changes into one set.
///
/// Line numbers are the git-native 1-indexed new-side lines; the gate converts
/// tree-sitter's 0-indexed fragment lines to match (see [`crate::gate`]).
pub fn changed_lines(dir: &Path, base: &str) -> Result<RepoDiff, DiffError> {
    let base_sha = merge_base(dir, base)?;
    // `--unified=0` so every hunk header's new-side range is exactly the
    // added/modified lines; `--no-ext-diff`/`--no-textconv` keep the payload a
    // literal unified diff. This covers tracked edits (committed since the base
    // and uncommitted), but git omits untracked files from `diff`.
    let raw = git(
        dir,
        &[
            "diff",
            "--no-color",
            "--no-ext-diff",
            "--no-textconv",
            "--unified=0",
            &base_sha,
        ],
    )?;
    let relative = parse_unified_diff(&raw)?;

    // Resolve git's repo-relative paths against the repository root so they can
    // be compared to clone-fragment paths (which reflect the scan target, not
    // the repo root).
    let root = repo_root(dir)?;
    let mut changed = ChangedLines(
        relative
            .0
            .into_iter()
            .map(|(rel, lines)| (absolutize(&root, &rel), lines))
            .collect(),
    );

    // A brand-new (untracked, non-ignored) file is an uncommitted change whose
    // every line is added, but `git diff` never reports it. List those files and
    // count all their lines so a duplicated new file cannot slip past the gate.
    add_untracked(dir, &root, &mut changed)?;

    Ok(RepoDiff { base_sha, changed })
}

/// The repository's top-level directory (canonicalized), the anchor for git's
/// repo-relative diff paths.
fn repo_root(dir: &Path) -> Result<PathBuf, DiffError> {
    let out = git(dir, &["rev-parse", "--show-toplevel"])?;
    Ok(PathBuf::from(out.trim()))
}

/// Join a repo-relative path onto the repo root and canonicalize it. Falls back
/// to the plain join if canonicalization fails (e.g. the file was since
/// deleted), which still matches a fragment path canonicalized the same way.
fn absolutize(root: &Path, rel: &Path) -> PathBuf {
    let joined = root.join(rel);
    std::fs::canonicalize(&joined).unwrap_or(joined)
}

/// Add every line of each untracked, non-`.gitignore`d file to `changed`. Each
/// such file is wholly new, so its changed lines are `1..=line_count`. Paths are
/// keyed absolutely (against `root`) to match [`changed_lines`].
fn add_untracked(dir: &Path, root: &Path, changed: &mut ChangedLines) -> Result<(), DiffError> {
    // `-z` gives NUL-separated paths so filenames with spaces/newlines survive;
    // `--exclude-standard` honors .gitignore/.git/info/exclude so ignored build
    // output is not counted as a change.
    let listing = git(dir, &["ls-files", "--others", "--exclude-standard", "-z"])?;

    for rel in listing.split('\0').filter(|s| !s.is_empty()) {
        let rel = Path::new(rel);
        let path = absolutize(root, rel);
        // A file listed by git but unreadable (a race, or a broken symlink) is
        // skipped rather than failing the whole gate: it contributes no lines.
        let Ok(contents) = std::fs::read_to_string(&path) else {
            continue;
        };
        let line_count = contents.lines().count();
        if line_count == 0 {
            continue;
        }
        let entry = changed.0.entry(path).or_default();
        entry.extend(1..=line_count);
    }

    Ok(())
}

/// Parse a `--unified=0 --no-color` diff into the set of added/modified new-side
/// line numbers per file. Pure: driven only by the text, so it is unit-tested
/// against fixed diff fixtures.
///
/// The two lines that matter:
/// - `+++ b/<path>` names the current file (`+++ /dev/null` on a deletion).
/// - `@@ -old[,n] +new[,m] @@` gives the new-side start `new` and length `m`
///   (`m` defaults to 1). Those `m` lines, `new..new+m`, are the changed lines.
pub fn parse_unified_diff(diff: &str) -> Result<ChangedLines, DiffError> {
    let mut out: BTreeMap<PathBuf, BTreeSet<usize>> = BTreeMap::new();
    let mut current: Option<PathBuf> = None;

    for line in diff.lines() {
        if let Some(rest) = line.strip_prefix("+++ ") {
            current = new_side_path(rest);
        } else if line.starts_with("@@ ") {
            // A hunk with no `+++` before it, or one against /dev/null
            // (deletion), has no new-side file to attribute lines to.
            let Some(path) = current.clone() else {
                continue;
            };
            let range = parse_hunk_new_range(line)?;
            let entry = out.entry(path).or_default();
            for offset in 0..range.count {
                entry.insert(range.start + offset);
            }
        }
    }

    Ok(ChangedLines(out))
}

/// Extract the new-side path from a `+++ ` header body, stripping the `b/`
/// prefix git adds. `/dev/null` (a deletion) yields `None`.
fn new_side_path(rest: &str) -> Option<PathBuf> {
    // Strip a trailing tab-delimited timestamp if present (git omits it, but
    // some diff producers add one).
    let path = rest.split('\t').next().unwrap_or(rest);
    if path == "/dev/null" {
        return None;
    }
    let stripped = path.strip_prefix("b/").unwrap_or(path);
    Some(PathBuf::from(stripped))
}

/// The new-side line range of a hunk header: `count` lines starting at 1-indexed
/// `start`. A `count` of 0 is a pure deletion (no new-side lines).
struct HunkRange {
    start: usize,
    count: usize,
}

/// Parse the new-side `+new[,count]` range from a hunk header
/// `@@ -old[,n] +new[,m] @@ ...`. A `count` of 0 (pure deletion at that point)
/// yields an empty range.
fn parse_hunk_new_range(line: &str) -> Result<HunkRange, DiffError> {
    let new_field = line
        .split_whitespace()
        .find_map(|token| token.strip_prefix('+'))
        .context(BadHunkHeaderSnafu { line })?;

    let mut parts = new_field.splitn(2, ',');
    let start: usize = parts
        .next()
        .and_then(|s| s.parse().ok())
        .context(BadHunkHeaderSnafu { line })?;
    let count: usize = match parts.next() {
        Some(c) => c.parse().ok().context(BadHunkHeaderSnafu { line })?,
        None => 1,
    };
    Ok(HunkRange { start, count })
}

#[cfg(test)]
mod tests;
