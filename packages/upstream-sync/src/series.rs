//! Reader for a fork repo's patch series: the commit DAG between the pinned
//! upstream base and the fork's megamerge bookmark.
//!
//! Since the jj megamerge migration each fork's series lives as real commits
//! in its GitHub fork repo (`lib/fork-packages.nix` `forkRepo`/`bookmark`):
//! every patch is a commit whose parents are its true dependencies, sealed by
//! an "ix megamerge" commit whose tree is the full series applied linearly.
//! This module opens a scratch commits-only clone, derives the series (see
//! [`series`]: the read depends on whether the branch still carries a
//! megamerge seal, and the base is anchored on the registry's `upstreamRef`
//! when it sits off the default branch), and exposes the ancestry closure
//! that decides what an upstream contribution drags along. Patch identity is
//! the commit SUBJECT, and the intent map in the registry is keyed by it.

use std::collections::HashSet;
use std::path::{Path, PathBuf};

use anstream::eprintln;
use color_eyre::eyre::{Result, WrapErr, eyre};
use lazy_regex::regex;

use crate::cmd;
use crate::mapping::Fork;
use crate::style::{YELLOW, paint};

/// One patch commit of the series.
#[derive(Debug, Clone)]
pub struct Commit {
    pub sha: String,
    pub subject: String,
}

/// A scratch clone of a fork repo with the series derived. The scratch dir
/// lives as long as this value and is removed on drop.
pub struct Repo {
    dir: PathBuf,
    _scratch: tempfile::TempDir,
    /// The bookmark tip (the megamerge seal commit).
    pub tip: String,
    /// merge-base(bookmark tip, tracked upstream branch): the pinned base
    /// the series sits on.
    pub base: String,
    /// The tracked upstream branch (the registry's `upstreamRef`, else the
    /// upstream's default branch). PRs target it.
    pub upstream_branch: String,
    /// The tracked upstream branch's tip at open time.
    pub upstream_tip: String,
    /// The patch series in topological order, parents first.
    pub series: Vec<Commit>,
}

impl Repo {
    /// Open a fork's series: scratch clone, fetch the bookmark tip from the
    /// fork repo and the default branch from the upstream, derive
    /// base + series.
    ///
    /// # Errors
    /// Fails when a fetch fails, the histories are unrelated (no merge
    /// base), or two series commits share a subject (subjects are the patch
    /// identity, so a duplicate would make the intent map ambiguous).
    pub fn open(fork: &Fork) -> Result<Self> {
        let scratch = tempfile::Builder::new()
            .prefix(&format!("upstream-sync-{}.", fork.name))
            .tempdir()
            .wrap_err("cannot create scratch dir")?;
        let dir = scratch.path().to_path_buf();
        cmd::run_in(&dir, "git", &["init", "--quiet"])?;
        neutralize_config(&dir)?;
        cmd::run_in(&dir, "git", &["remote", "add", "fork", &fork.fork_url()])?;
        cmd::run_in(
            &dir,
            "git",
            &["remote", "add", "upstream", &fork.upstream_url],
        )?;

        let tip = fetch(&dir, &fork.name, "fork", &format!("refs/heads/{}", fork.bookmark))?;
        // A fork based off a non-default branch declares `upstreamRef` in
        // the registry (nix's 2.34.7 base sits on 2.34-maintenance):
        // merge-basing against the default branch would undershoot the base
        // and pull upstream commits into the series (#4038).
        let upstream_branch = match &fork.upstream_ref {
            Some(configured) => configured.clone(),
            None => default_branch(&dir, &fork.name, &fork.upstream_url)?,
        };
        let upstream_tip = fetch(
            &dir,
            &fork.name,
            "upstream",
            &format!("refs/heads/{upstream_branch}"),
        )?;

        let base = cmd::run_in(&dir, "git", &["merge-base", &tip, &upstream_tip])
            .wrap_err_with(|| {
                format!(
                    "{}: no merge base between {}@{} and {}@{upstream_branch}; the fork is not a fork of that upstream",
                    fork.name, fork.fork_repo, fork.bookmark, fork.upstream_url
                )
            })?;

        let series = series(&dir, &fork.name, &tip, &base)?;
        Ok(Self {
            dir,
            _scratch: scratch,
            tip,
            base,
            upstream_branch,
            upstream_tip,
            series,
        })
    }

    /// The series' subjects in series order.
    #[must_use]
    pub fn subjects(&self) -> Vec<String> {
        self.series.iter().map(|c| c.subject.clone()).collect()
    }

    /// The series commit with this exact subject.
    #[must_use]
    pub fn find(&self, subject: &str) -> Option<&Commit> {
        self.series.iter().find(|c| c.subject == subject)
    }

    /// A commit's message body (everything after the subject).
    ///
    /// # Errors
    /// Fails when git cannot read the commit.
    pub fn body(&self, sha: &str) -> Result<String> {
        cmd::run_in(&self.dir, "git", &["log", "-1", "--format=%b", sha])
    }

    /// The contribution closure of a patch commit: its ancestry minus the
    /// base's, in topological order with the patch itself last. The commit
    /// DAG IS the dependency graph, so this is exactly the set of series
    /// commits an upstream PR for `sha` carries.
    ///
    /// # Errors
    /// Fails when git cannot walk the range.
    pub fn closure(&self, sha: &str) -> Result<Vec<Commit>> {
        let out = cmd::run_in(
            &self.dir,
            "git",
            &[
                "log",
                "--topo-order",
                "--reverse",
                "--format=%H%x1f%s",
                sha,
                &format!("^{}", self.base),
            ],
        )?;
        Ok(parse_log(&out))
    }

    /// Does `sha` still merge into the tracked upstream branch?
    ///
    /// Returns the conflicting paths, or `None` when the merge is clean.
    ///
    /// Asked before the outward act because a patch is written against the
    /// base its fork sat on, and that base ages. On 2026-07-27 the
    /// home-manager patch was 45 commits behind master, both files it
    /// touched had moved, and the PR it opened was dead on arrival: the
    /// upstream CI reported "merge conflicts with base branch" before any
    /// human read it. An unattended lane doing that repeatedly is how a
    /// contributor gets ignored.
    ///
    /// This forces trees into the scratch clone, which the series reader
    /// deliberately avoids fetching. That is the price of the check, and it
    /// is paid once, only for a patch about to be submitted.
    ///
    /// # Errors
    /// Fails when git cannot run the merge.
    pub fn conflicts_with_upstream(&self, sha: &str) -> Result<Option<String>> {
        let out = cmd::complete_in(
            &self.dir,
            "git",
            &[
                "merge-tree",
                "--write-tree",
                "--name-only",
                &self.upstream_tip,
                sha,
            ],
        )?;
        // git 2.38+: 0 clean, 1 conflicted, >1 a real error. A broken check
        // must not read as a clean merge, so anything else is an error.
        match out.status {
            0 => Ok(None),
            1 => Ok(Some(
                out.stdout
                    .lines()
                    .skip_while(|l| !l.is_empty())
                    .collect::<Vec<_>>()
                    .join("\n")
                    .trim()
                    .to_owned(),
            )),
            _ => Err(eyre!(
                "cannot test-merge {sha} into {}: {}",
                self.upstream_branch,
                out.stderr.trim()
            )),
        }
    }

    /// Force-push `sha` to `refs/heads/<branch>` on the fork repo. Pushing
    /// to OUR fork is not the outward act; force because a jj rebase
    /// rewrites the series by design.
    ///
    /// # Errors
    /// Fails when the push fails.
    pub fn push_branch(&self, sha: &str, branch: &str) -> Result<()> {
        cmd::run_in(
            &self.dir,
            "git",
            &[
                "push",
                "--force",
                "--quiet",
                "fork",
                &format!("{sha}:refs/heads/{branch}"),
            ],
        )?;
        Ok(())
    }
}

/// Deterministic scratch-repo config so a developer's global git settings do
/// not perturb reads or pushes.
fn neutralize_config(dir: &Path) -> Result<()> {
    for (key, value) in [
        ("commit.gpgsign", "false"),
        ("core.autocrlf", "false"),
        ("advice.detachedHead", "false"),
    ] {
        cmd::run_in(dir, "git", &["config", key, value])?;
    }
    Ok(())
}

/// Fetch one ref from a remote, commits-only (`--filter=tree:0`): the series
/// reader needs commit metadata and graph shape, never file contents.
/// Servers without `uploadpack.allowFilter` (plain local repos in tests)
/// reject the filter; degrade to a full fetch with a warning rather than
/// fail.
fn fetch(dir: &Path, fork: &str, remote: &str, refspec: &str) -> Result<String> {
    let filtered = cmd::complete_in(
        dir,
        "git",
        &["fetch", "--quiet", "--filter=tree:0", remote, refspec],
    )?;
    if !filtered.ok() {
        eprintln!(
            "{}",
            paint(
                YELLOW,
                &format!(
                    "upstream-sync: {fork}: {remote} does not support commits-only fetch (--filter=tree:0); falling back to a full fetch"
                )
            )
        );
        cmd::run_in(dir, "git", &["fetch", "--quiet", remote, refspec])
            .wrap_err_with(|| format!("{fork}: cannot fetch {refspec} from {remote}"))?;
    }
    cmd::run_in(dir, "git", &["rev-parse", "FETCH_HEAD"])
}

/// Discover the upstream's default branch (its HEAD symref): contributions
/// target it, not our pinned base.
fn default_branch(dir: &Path, fork: &str, url: &str) -> Result<String> {
    let symref = cmd::run_in(dir, "git", &["ls-remote", "--symref", "upstream", "HEAD"])?;
    symref
        .lines()
        .find(|l| l.starts_with("ref:"))
        .and_then(|l| regex!(r"ref:\s+refs/heads/(\S+)\s+HEAD").captures(l))
        .map(|c| c[1].to_owned())
        .ok_or_else(|| eyre!("{fork}: cannot discover the default branch of {url}"))
}

/// The series: the fork's patch commits between the base and the bookmark,
/// topo order.
///
/// Two branch shapes exist and they need opposite reads, so the shape is
/// detected rather than assumed.
///
/// A branch still in the jj megamerge shape ends in an "ix megamerge" seal
/// whose PARENTS are the patches (5 of them on rust-clippy), and a patch there
/// can itself be a merge commit, because a patch's parents are its declared
/// dependencies. Neither `--first-parent` nor `--no-merges` is safe on that
/// shape: both drop real patches. So it keeps the full-ancestry walk, minus
/// the seal by subject.
///
/// A branch in the merge-forward shape the `forkBranches` doctrine mandates
/// has no seal: patches are ordinary commits on it, upstream is merged in
/// rather than rebased onto, and an earlier revision of a patch may be merged
/// back so a rev some flake.lock pinned stays reachable. There the full walk
/// reads the merge commits as patches (offering to upstream "Merge
/// nix-community/home-manager master") and reads those earlier revisions as
/// extra patches, which is fatal because patch identity is the subject and two
/// revisions of one patch usually share one. `--first-parent --no-merges` is
/// the branch's own line, and it is the whole series only while every patch
/// reaches the branch as a commit ON it.
///
/// That last condition does not hold, so the branch's own line is the SPINE
/// and [`recover_merged_patches`] adds back the patches that arrived on a
/// merge's second parent. Both flags stay: dropping either is what read three
/// revisions of one home-manager patch as three patches (ENG-11646).
///
/// The megamerge arm is transitional: megamerges are banned, and it becomes
/// dead code to delete once the last fork is migrated (ENG-11665).
fn series(dir: &Path, fork: &str, tip: &str, base: &str) -> Result<Vec<Commit>> {
    let range = format!("^{base}");
    let full = cmd::run_in(
        dir,
        "git",
        &[
            "log",
            "--topo-order",
            "--reverse",
            "--format=%H%x1f%s",
            tip,
            &range,
        ],
    )?;
    let all = parse_log(&full);
    let series: Vec<Commit> = if all.iter().any(is_seal) {
        all.into_iter().filter(|c| !is_seal(c)).collect()
    } else {
        let out = cmd::run_in(
            dir,
            "git",
            &[
                "log",
                "--topo-order",
                "--reverse",
                "--first-parent",
                "--no-merges",
                "--format=%H%x1f%s",
                tip,
                &range,
            ],
        )?;
        let spine = parse_log(&out);
        let recovered = recover_merged_patches(dir, fork, tip, &range, &all, &spine)?;
        if recovered.is_empty() {
            spine
        } else {
            let keep: HashSet<&str> = spine
                .iter()
                .chain(&recovered)
                .map(|c| c.sha.as_str())
                .collect();
            // Filtering the full walk rather than concatenating puts each
            // recovered patch in its real topological place among the
            // branch's own commits, which is the order the series promises.
            all.iter()
                .filter(|c| keep.contains(c.sha.as_str()))
                .cloned()
                .collect()
        }
    };

    // Subjects are the patch identity (intent keys, branch slugs); a
    // duplicate would make every subject-keyed lookup ambiguous.
    let mut seen: Vec<&str> = Vec::new();
    for commit in &series {
        if seen.contains(&commit.subject.as_str()) {
            return Err(eyre!(
                "{fork}: duplicate commit subject in the series: '{}'. Subjects are the patch identity; retitle one of the commits.",
                commit.subject
            ));
        }
        seen.push(&commit.subject);
    }
    Ok(series)
}

/// The patches that reached the branch on a merge's second parent, which the
/// spine walk cannot see.
///
/// A patch that arrived as a merged pull request sits off the branch's own
/// line, so `--first-parent` never reaches it and `--no-merges` drops the
/// merge that would have led there. It is then absent rather than
/// unclassified, which is the worse of the two: an unclassified patch defaults
/// to `hold` and one intent entry fixes it, while an entry naming an absent
/// one is an orphaned key that `ensure_no_orphaned_intent` rejects, so the gap
/// resists being documented. On indexable-inc/nix that is 22 patches across 7
/// merged pull requests, and on indexable-inc/jj 11 across 5 (ENG-11686).
///
/// Recovering them cannot mean walking second parents generally, which is the
/// fence `--first-parent` puts up and ENG-11646 paid for. Three filters keep
/// it, and each one is load-bearing on a fork we actually have:
///
///  1. Only merges that CHANGED THE TREE. The doctrine's merge-back of an
///     earlier revision, so a rev some flake.lock still pins stays reachable,
///     is a `-s ours` merge whose tree equals its first parent's: it carried
///     ancestry, not a patch. home-manager has two, and reading them as
///     patches is what ENG-11646 was.
///  2. Nothing already on the spine, by SHA.
///  3. Nothing whose SUBJECT is already on the spine. An earlier revision
///     under a merge that did change the tree still is not a second patch;
///     the branch's own copy represents it. indexable-inc/git has exactly
///     this, and the two commits share a patch-id.
///
/// Upstream's own commits need no filter: `base` is `merge-base(tip,
/// upstream)`, so everything upstream merged forward is already behind it.
///
/// The set is derived by COMPARING THE WALKS rather than by asserting a
/// count, so this cannot pass by both walks returning the same wrong thing.
/// It warns as well as recovering, because the shape is against
/// `forkBranches` -- a change is meant to land as a commit on the branch --
/// and a silently absorbed merge is one nobody stops producing.
fn recover_merged_patches(
    dir: &Path,
    fork: &str,
    tip: &str,
    range: &str,
    all: &[Commit],
    spine: &[Commit],
) -> Result<Vec<Commit>> {
    let merges = cmd::run_in(dir, "git", &["rev-list", "--merges", tip, range])?;
    let merge_shas: HashSet<&str> = merges.split_whitespace().collect();
    let spine_shas: HashSet<&str> = spine.iter().map(|c| c.sha.as_str()).collect();
    let spine_subjects: HashSet<&str> = spine.iter().map(|c| c.subject.as_str()).collect();
    let brought = commits_a_merge_brought_in(dir, tip, range)?;

    let recovered: Vec<Commit> = all
        .iter()
        .filter(|c| {
            brought.contains(c.sha.as_str())
                && !spine_shas.contains(c.sha.as_str())
                && !merge_shas.contains(c.sha.as_str())
                && !spine_subjects.contains(c.subject.as_str())
        })
        .cloned()
        .collect();
    if recovered.is_empty() {
        return Ok(recovered);
    }

    let found = recovered.len();
    let listed = recovered
        .iter()
        .map(|c| format!("  - {}", c.subject))
        .collect::<Vec<_>>()
        .join("\n");
    eprintln!(
        "{}",
        paint(
            YELLOW,
            &format!(
                "upstream-sync: {fork}: {found} patch(es) reached the branch on a merge commit's \
                 second parent rather than as a commit on it, which is against `forkBranches`. \
                 They are in the series, so they can be classified and offered upstream, but land \
                 the next one as a commit on the branch (ENG-11686):\n{listed}"
            )
        )
    );
    Ok(recovered)
}

/// The commits each merge on the branch's own line brought in with it,
/// skipping the merges that brought in no content.
///
/// A merge whose tree equals its first parent's changed nothing, so its second
/// parent is ancestry rather than a patch: that is the `-s ours` merge-back
/// keeping a pinned revision reachable. Only merges on the FIRST-PARENT line
/// are walked; a merge deeper in is already inside the range one of those
/// contributes.
fn commits_a_merge_brought_in(dir: &Path, tip: &str, range: &str) -> Result<HashSet<String>> {
    let merges = cmd::run_in(
        dir,
        "git",
        &["rev-list", "--first-parent", "--merges", tip, range],
    )?;
    let mut brought = HashSet::new();
    for merge in merges.split_whitespace() {
        let tree = cmd::run_in(dir, "git", &["rev-parse", &format!("{merge}^{{tree}}")])?;
        let first = cmd::run_in(dir, "git", &["rev-parse", &format!("{merge}^1^{{tree}}")])?;
        if tree == first {
            continue;
        }
        let side = cmd::run_in(
            dir,
            "git",
            &["rev-list", &format!("{merge}^2"), &format!("^{merge}^1")],
        )?;
        brought.extend(side.split_whitespace().map(ToOwned::to_owned));
    }
    Ok(brought)
}

/// The megamerge seal: bookkeeping (tree = series applied linearly), not a
/// patch. Its presence is also what says the branch is still in the megamerge
/// shape.
fn is_seal(commit: &Commit) -> bool {
    commit.subject.starts_with("ix megamerge")
}

/// Parse `--format=%H%x1f%s` output.
fn parse_log(out: &str) -> Vec<Commit> {
    out.lines()
        .filter_map(|line| line.split_once('\u{1f}'))
        .map(|(sha, subject)| Commit {
            sha: sha.to_owned(),
            subject: subject.to_owned(),
        })
        .collect()
}

/// A filesystem/branch-safe slug from a patch subject, e.g.
/// `fix(libstore): don't crash` -> `fix-libstore-don-t-crash`.
#[must_use]
pub fn slug(subject: &str) -> String {
    regex!(r"[^a-z0-9]+")
        .replace_all(&subject.to_lowercase(), "-")
        .trim_matches('-')
        .to_owned()
}

/// Resolve a user-provided patch reference to an exact subject: exact match,
/// else unique prefix, else unique substring.
///
/// # Errors
/// Fails when nothing matches, or when the reference is ambiguous.
pub fn resolve(reference: &str, subjects: &[String]) -> Result<String> {
    if subjects.iter().any(|s| s == reference) {
        return Ok(reference.to_owned());
    }
    let by_prefix: Vec<&String> = subjects
        .iter()
        .filter(|s| s.starts_with(reference))
        .collect();
    if let [only] = by_prefix.as_slice() {
        return Ok((*only).clone());
    }
    let by_sub: Vec<&String> = subjects.iter().filter(|s| s.contains(reference)).collect();
    if let [only] = by_sub.as_slice() {
        return Ok((*only).clone());
    }
    let mut candidates: Vec<&String> = by_prefix;
    for c in by_sub {
        if !candidates.contains(&c) {
            candidates.push(c);
        }
    }
    if candidates.is_empty() {
        return Err(eyre!(
            "upstream-pr: no patch matching '{reference}'. Known: {}",
            subjects.join(", ")
        ));
    }
    Err(eyre!(
        "upstream-pr: '{reference}' is ambiguous; matches: {}",
        candidates
            .iter()
            .map(|c| c.as_str())
            .collect::<Vec<_>>()
            .join(", ")
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn slug_is_branch_safe() {
        assert_eq!(
            slug("fix(libstore): don't crash the daemon"),
            "fix-libstore-don-t-crash-the-daemon"
        );
        assert_eq!(slug("Add.Thing_Now"), "add-thing-now");
    }

    #[test]
    fn resolve_exact_prefix_substring_ambiguous() {
        let subjects: Vec<String> = ["alpha: one", "beta: two", "beta: twenty"]
            .map(str::to_owned)
            .to_vec();
        assert_eq!(resolve("alpha: one", &subjects).unwrap(), "alpha: one");
        assert_eq!(resolve("alph", &subjects).unwrap(), "alpha: one");
        assert_eq!(resolve("twenty", &subjects).unwrap(), "beta: twenty");
        assert!(resolve("beta", &subjects).is_err());
        assert!(resolve("zzz", &subjects).is_err());
    }

    #[test]
    fn log_parse_and_seal_filter() {
        let commits = parse_log("aaa\u{1f}patch one\nbbb\u{1f}ix megamerge: 2 patches on abc\n");
        assert_eq!(commits.len(), 2);
        let series: Vec<Commit> = commits
            .into_iter()
            .filter(|c| !c.subject.starts_with("ix megamerge"))
            .collect();
        assert_eq!(series.len(), 1);
        assert_eq!(series[0].subject, "patch one");
    }
}
