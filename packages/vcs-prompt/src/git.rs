//! Branch, upstream distance, and worktree status of a git repository, read
//! from one `git status --porcelain=v2 --branch`.
//!
//! Asking git rather than linking libgit2: on the config repo here (nested
//! submodules, one of them huge) git answers in ~60ms where the equivalent
//! libgit2 status walk takes ~180ms, git honors whatever the user configured
//! for status, and the crate keeps its build free of a vendored C library.

use std::path::Path;
use std::process::Command;

use color_eyre::eyre::{Result, WrapErr, eyre};

/// What HEAD points at.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HeadName {
    /// A branch, named even when it has no commits yet.
    Branch(String),
    /// Detached HEAD, carrying the abbreviated commit id.
    Detached(String),
}

/// Commits HEAD has that its upstream does not, and the other way round.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Tracking {
    pub ahead: usize,
    pub behind: usize,
}

/// Path counts, grouped the way starship's `git_status` groups them so the
/// segment keeps the vocabulary this prompt already had.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Counts {
    pub conflicted: usize,
    pub deleted: usize,
    pub renamed: usize,
    pub modified: usize,
    pub staged: usize,
    pub untracked: usize,
}

/// Everything the prompt shows about a git repository.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Head {
    pub name: HeadName,
    /// `None` when the branch has no upstream, or when HEAD is detached.
    pub tracking: Option<Tracking>,
    pub counts: Counts,
}

/// Read the repository at `root`.
pub fn head(root: &Path) -> Result<Head> {
    // `--no-optional-locks` keeps a prompt render from taking the index lock
    // to refresh caches, which would collide with a git command running in
    // another pane. Ignored files are not asked for; the segment has no
    // count for them.
    let output = Command::new("git")
        .arg("-C")
        .arg(root)
        .args([
            "--no-optional-locks",
            "status",
            "--porcelain=v2",
            "--branch",
        ])
        .output()
        .wrap_err("failed to run `git status`")?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(eyre!("`git status` failed: {}", stderr.trim()));
    }

    let stdout = String::from_utf8(output.stdout).wrap_err("`git status` wrote non-UTF-8 output")?;
    parse(&stdout)
}

/// Length of the abbreviated commit id shown for a detached HEAD, matching
/// git's own default `core.abbrev` floor.
const SHORT_ID: usize = 7;

/// The name porcelain v2 uses for a HEAD that is on no branch.
const DETACHED: &str = "(detached)";

fn parse(stdout: &str) -> Result<Head> {
    let mut branch = None;
    let mut oid = None;
    let mut tracking = None;
    let mut counts = Counts::default();

    for line in stdout.lines() {
        match line.strip_prefix("# branch.") {
            Some(header) => match header.split_once(' ') {
                Some(("head", value)) => branch = Some(value),
                Some(("oid", value)) => oid = Some(value),
                Some(("ab", value)) => tracking = parse_tracking(value),
                _ => {}
            },
            None => count(line, &mut counts),
        }
    }

    let branch = branch.ok_or_else(|| eyre!("`git status` printed no `# branch.head` header"))?;
    let name = if branch == DETACHED {
        let oid = oid.ok_or_else(|| eyre!("`git status` printed no `# branch.oid` header"))?;
        HeadName::Detached(oid.chars().take(SHORT_ID).collect())
    } else {
        HeadName::Branch(branch.to_owned())
    };

    Ok(Head {
        name,
        tracking,
        counts,
    })
}

/// `+2 -1` from the `# branch.ab` header. The header is absent entirely when
/// the branch has no upstream, so a malformed one is the only `None` here.
fn parse_tracking(value: &str) -> Option<Tracking> {
    let (ahead, behind) = value.split_once(' ')?;
    Some(Tracking {
        ahead: ahead.strip_prefix('+')?.parse().ok()?,
        behind: behind.strip_prefix('-')?.parse().ok()?,
    })
}

/// Fold one porcelain v2 entry into the counts.
///
/// Ordinary (`1`) and rename/copy (`2`) entries carry a two-letter `XY` field:
/// `X` is what is staged, `Y` is what the worktree has on top. Both halves are
/// counted, so a path staged and then edited again shows up as staged *and*
/// modified, the way `git_status` reported it.
fn count(line: &str, counts: &mut Counts) {
    let mut fields = line.split(' ');
    match fields.next() {
        Some("?") => counts.untracked += 1,
        Some("u") => counts.conflicted += 1,
        Some("1" | "2") => {
            let Some(xy) = fields.next() else { return };
            let mut states = xy.chars();
            let (Some(staged), Some(worktree)) = (states.next(), states.next()) else {
                return;
            };

            match staged {
                'R' | 'C' => counts.renamed += 1,
                'M' | 'T' | 'A' | 'D' => counts.staged += 1,
                _ => {}
            }
            match worktree {
                'R' | 'C' => counts.renamed += 1,
                'M' | 'T' | 'A' => counts.modified += 1,
                'D' => counts.deleted += 1,
                _ => {}
            }
        }
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use super::{Counts, HeadName, Tracking, parse};

    /// A dirty submodule, a staged rename, an unmerged path, and an untracked
    /// file, in the shape `git status --porcelain=v2 --branch` emits.
    const STATUS: &str = "\
# branch.oid dc3a5af9332bf3f46fd7624e49d55f19d668b344
# branch.head main
# branch.upstream origin/main
# branch.ab +2 -1
1 .M S.MU 160000 160000 160000 7b87236e 7b87236e ix
2 R. N... 100644 100644 100644 1e4f2a0b 1e4f2a0b R100 new.rs\told.rs
u UU N... 100644 100644 100644 100644 0000 1111 2222 conflict.rs
? notes.md
";

    #[test]
    fn reads_the_branch_upstream_distance_and_counts() {
        let head = parse(STATUS).expect("parse status");

        assert_eq!(head.name, HeadName::Branch("main".to_owned()));
        assert_eq!(head.tracking, Some(Tracking { ahead: 2, behind: 1 }));
        assert_eq!(
            head.counts,
            Counts {
                conflicted: 1,
                renamed: 1,
                modified: 1,
                untracked: 1,
                ..Counts::default()
            }
        );
    }

    #[test]
    fn a_detached_head_reports_the_abbreviated_commit() {
        let head = parse("# branch.oid dc3a5af9332bf3f46fd7624e49d55f19d668b344\n# branch.head (detached)\n")
            .expect("parse status");

        assert_eq!(head.name, HeadName::Detached("dc3a5af".to_owned()));
        assert_eq!(head.tracking, None);
    }

    #[test]
    fn a_branch_with_no_upstream_has_no_distance() {
        let head = parse("# branch.oid (initial)\n# branch.head main\n").expect("parse status");

        assert_eq!(head.name, HeadName::Branch("main".to_owned()));
        assert_eq!(head.tracking, None);
        assert_eq!(head.counts, Counts::default());
    }

    #[test]
    fn output_without_a_branch_header_is_an_error() {
        assert!(parse("? notes.md\n").is_err());
    }
}
