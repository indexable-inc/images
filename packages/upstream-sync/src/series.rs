//! Reader for a fork repo's patch series: the commit DAG between the pinned
//! upstream base and the fork's megamerge bookmark.
//!
//! Since the jj megamerge migration each fork's series lives as real commits
//! in its GitHub fork repo (`lib/fork-packages.nix` `forkRepo`/`bookmark`):
//! every patch is a commit whose parents are its true dependencies, sealed by
//! an "ix megamerge" commit whose tree is the full series applied linearly.
//! This module opens a scratch commits-only clone, derives the series
//! (bookmark ancestry minus the upstream base -- anchored on the registry's
//! `upstreamRef` when the base sits off the default branch -- minus the
//! seal), and exposes
//! the ancestry closure that decides what an upstream contribution drags
//! along. Patch identity is the commit SUBJECT: it survives jj rebases, and
//! the intent map in the registry is keyed by it.

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

/// The series: bookmark ancestry minus the base, topo order, minus the
/// megamerge seal commit (subject "ix megamerge: ..."), which is bookkeeping
/// (tree = series applied linearly), not a patch.
fn series(dir: &Path, fork: &str, tip: &str, base: &str) -> Result<Vec<Commit>> {
    let out = cmd::run_in(
        dir,
        "git",
        &[
            "log",
            "--topo-order",
            "--reverse",
            "--format=%H%x1f%s",
            tip,
            &format!("^{base}"),
        ],
    )?;
    let series: Vec<Commit> = parse_log(&out)
        .into_iter()
        .filter(|c| !c.subject.starts_with("ix megamerge"))
        .collect();

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
