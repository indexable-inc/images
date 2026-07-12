//! Find every patch carried against a vendored dependency and its associated
//! upstream PR.
//!
//! Three sources, most authoritative first:
//!
//! 1. The fork registry (`lib/fork-packages.nix`), consumed as the same
//!    rendered JSON `upstream-sync` reads. Each entry names a patch series
//!    directory and per-patch intent; an optional per-patch `pr` field is an
//!    explicit PR-URL override.
//! 2. The tool-owned `upstream-status.json` next to each series, where
//!    `upstream-sync` records the PRs it opened and tracks.
//! 3. The patch files themselves: the first `https://github.com/<o>/<r>/pull/<n>`
//!    URL in a patch's header (its commit message, before the first diff hunk).
//!
//! Loose `*.patch` files outside any registered series (ad-hoc
//! `patches = [ ... ]` overrides) are discovered by walking the repo tree, so
//! the view covers ALL packages, not just the de-forked ones.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::Command;

use color_eyre::eyre::{Result, WrapErr, eyre};
use lazy_regex::regex_captures;
use serde::Deserialize;

use crate::model::{PatchRow, PrRef, PrSource};

/// Env var the nix wrapper sets to the rendered `fork-packages.json`.
pub const MAPPING_ENV: &str = "PRS_FORK_MAPPING";

/// One fork entry from the rendered `lib/fork-packages.nix` registry. Fields
/// the viewer does not use (upstream URL, policy, ...) are ignored.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Fork {
    pub name: String,
    pub patch_dir: String,
    #[serde(default)]
    pub patches: BTreeMap<String, PatchIntent>,
}

/// Per-patch declarative intent from the registry.
#[derive(Debug, Deserialize)]
pub struct PatchIntent {
    pub upstream: String,
    /// Optional explicit PR URL for a patch whose PR is not tracked in
    /// `upstream-status.json` (e.g. a pre-existing upstream PR).
    #[serde(default)]
    pub pr: Option<String>,
}

/// `upstream-status.json`: the live state `upstream-sync` owns.
#[derive(Debug, Deserialize)]
struct StatusFile {
    #[serde(default)]
    patches: BTreeMap<String, StatusPatch>,
}

#[derive(Debug, Deserialize)]
struct StatusPatch {
    pr: Option<StatusPr>,
}

#[derive(Debug, Deserialize)]
struct StatusPr {
    url: String,
}

/// A PR reference plus which source claimed it.
struct FoundPr {
    pr: PrRef,
    source: PrSource,
}

/// Locate the repo root: the explicit `--repo` value, else the nearest
/// ancestor of the working directory containing `lib/fork-packages.nix`.
pub fn repo_root(explicit: Option<PathBuf>) -> Option<PathBuf> {
    if explicit.is_some() {
        return explicit;
    }
    let mut dir = std::env::current_dir().ok()?;
    loop {
        if dir.join("lib/fork-packages.nix").is_file() {
            return Some(dir);
        }
        if !dir.pop() {
            return None;
        }
    }
}

/// Load the fork mapping: `--mapping` flag, else the `PRS_FORK_MAPPING` env
/// var the nix wrapper bakes, else evaluate `lib/fork-packages.nix` with the
/// `nix` on PATH (the cargo-run development path).
pub fn load_mapping(flag: Option<&Path>, root: Option<&Path>) -> Result<Vec<Fork>> {
    let env_path = std::env::var_os(MAPPING_ENV).map(PathBuf::from);
    let path = flag.map(Path::to_path_buf).or(env_path);
    if let Some(path) = path {
        let text = std::fs::read_to_string(&path)
            .wrap_err_with(|| format!("reading fork mapping {}", path.display()))?;
        return serde_json::from_str(&text)
            .wrap_err_with(|| format!("parsing fork mapping {}", path.display()));
    }
    let root = root.ok_or_else(|| {
        eyre!("no fork mapping: pass --mapping, set {MAPPING_ENV}, or run inside the repo")
    })?;
    let output = Command::new("nix")
        .args(["eval", "--json", "--file"])
        .arg(root.join("lib/fork-packages.nix"))
        .arg("forkPackages")
        .output()
        .wrap_err("running `nix eval` to render lib/fork-packages.nix (is nix on PATH?)")?;
    if !output.status.success() {
        return Err(eyre!(
            "`nix eval` on lib/fork-packages.nix failed: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }
    serde_json::from_slice(&output.stdout).wrap_err("parsing `nix eval` fork mapping output")
}

/// Parse a GitHub PR URL. Anything that is not `github.com/<o>/<r>/pull/<n>`
/// (a GitLab MR, an issue link) is not a PR reference.
pub fn parse_pr_url(url: &str) -> Option<PrRef> {
    let (whole, owner, repo, number) = regex_captures!(
        r"https://github\.com/([A-Za-z0-9_.-]+)/([A-Za-z0-9_.-]+)/pull/(\d+)",
        url
    )?;
    Some(PrRef {
        owner: owner.to_owned(),
        repo: repo.to_owned(),
        number: number.parse().ok()?,
        url: whole.to_owned(),
    })
}

/// Everything before the first `diff --git` line: the `git format-patch` mail
/// headers and commit message. A raw `git diff` patch has no header and may
/// START with `diff --git`, so a leading boundary yields an empty header
/// instead of scanning the diff body for PR URLs.
fn patch_header(text: &str) -> &str {
    if text.starts_with("diff --git ") {
        return "";
    }
    text.split("\ndiff --git ").next().unwrap_or("")
}

/// First PR URL in a patch's header.
fn header_pr(path: &Path) -> Option<PrRef> {
    let text = std::fs::read_to_string(path).ok()?;
    patch_header(&text).lines().find_map(parse_pr_url)
}

fn is_patch_file(path: &Path) -> bool {
    path.extension().is_some_and(|ext| ext == "patch")
}

/// Sorted `*.patch` file names directly inside `dir`.
fn patch_files(dir: &Path) -> Vec<String> {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut names: Vec<String> = entries
        .filter_map(Result::ok)
        .filter(|entry| is_patch_file(&entry.path()))
        .filter_map(|entry| entry.file_name().into_string().ok())
        .collect();
    names.sort();
    names
}

/// Tracked PR URLs from a series' `upstream-status.json`, keyed by patch file
/// name. Absent or unreadable file means nothing is tracked yet.
fn tracked_prs(patch_dir: &Path) -> BTreeMap<String, String> {
    let path = patch_dir.join("upstream-status.json");
    let Ok(text) = std::fs::read_to_string(&path) else {
        return BTreeMap::new();
    };
    let Ok(status) = serde_json::from_str::<StatusFile>(&text) else {
        return BTreeMap::new();
    };
    status
        .patches
        .into_iter()
        .filter_map(|(name, patch)| Some((name, patch.pr?.url)))
        .collect()
}

/// Resolve one registered patch's PR by source priority.
fn registry_pr(
    fork: &Fork,
    dir: Option<&Path>,
    tracked: &BTreeMap<String, String>,
    file: &str,
) -> Option<FoundPr> {
    let mapped = fork
        .patches
        .get(file)
        .and_then(|intent| intent.pr.as_deref())
        .and_then(parse_pr_url);
    if let Some(pr) = mapped {
        return Some(FoundPr {
            pr,
            source: PrSource::Mapping,
        });
    }
    if let Some(pr) = tracked.get(file).and_then(|url| parse_pr_url(url)) {
        return Some(FoundPr {
            pr,
            source: PrSource::Status,
        });
    }
    let pr = header_pr(&dir?.join(file))?;
    Some(FoundPr {
        pr,
        source: PrSource::PatchHeader,
    })
}

fn registry_rows(root: Option<&Path>, fork: &Fork) -> Vec<PatchRow> {
    let dir = root.map(|root| root.join(&fork.patch_dir));
    let tracked = dir.as_deref().map_or_else(BTreeMap::new, tracked_prs);
    // The on-disk series is the source of truth for which patches exist; the
    // mapping keys are only a fallback for the no-series-dir case (mapping
    // baked by nix, run outside a checkout). Never merge the two: a stale
    // mapping entry for a removed patch must not render a phantom row.
    let mut names: Vec<String> = dir
        .as_deref()
        .filter(|dir| dir.is_dir())
        .map_or_else(|| fork.patches.keys().cloned().collect(), patch_files);
    names.sort();
    names
        .into_iter()
        .map(|file| {
            let found = registry_pr(fork, dir.as_deref(), &tracked, &file);
            let path = dir
                .as_deref()
                .map(|dir| dir.join(&file))
                .filter(|path| path.is_file());
            PatchRow {
                fork: fork.name.clone(),
                // A registered patch with no mapping entry is `hold` with an
                // "unclassified" reason per the registry fail-safe (see
                // lib/fork-packages.nix); only loose patches carry no intent.
                intent: Some(
                    fork.patches
                        .get(&file)
                        .map_or_else(|| "hold".to_owned(), |entry| entry.upstream.clone()),
                ),
                pr: found.as_ref().map(|found| found.pr.clone()),
                pr_source: found.map(|found| found.source),
                status: None,
                path,
                dir: dir.clone().filter(|dir| dir.is_dir()),
                file,
            }
        })
        .collect()
}

/// Walk the repo for `*.patch` files outside every registered series (ad-hoc
/// `patches = [ ... ]` package overrides). Test fixtures are not dependency
/// patches, so `tests/` directories are skipped.
fn loose_rows(root: &Path, forks: &[Fork]) -> Vec<PatchRow> {
    let registered: Vec<PathBuf> = forks
        .iter()
        .map(|fork| root.join(&fork.patch_dir))
        .collect();
    let mut rows = Vec::new();
    let walk = ignore::WalkBuilder::new(root)
        .filter_entry(|entry| entry.file_name() != "tests")
        .build();
    for entry in walk.filter_map(Result::ok) {
        let path = entry.path();
        if !is_patch_file(path) || registered.iter().any(|dir| path.starts_with(dir)) {
            continue;
        }
        let Some(parent) = path.parent() else {
            continue;
        };
        // Name the row after the containing package, with a trailing
        // `patches/` directory folded away.
        let package = match parent.file_name().and_then(std::ffi::OsStr::to_str) {
            Some("patches") => parent.parent().unwrap_or(parent),
            _ => parent,
        };
        let fork = package
            .strip_prefix(root)
            .unwrap_or(package)
            .display()
            .to_string();
        let pr = header_pr(path);
        rows.push(PatchRow {
            fork,
            file: path
                .file_name()
                .map_or_else(String::new, |name| name.to_string_lossy().into_owned()),
            intent: None,
            pr_source: pr.as_ref().map(|_| PrSource::PatchHeader),
            pr,
            status: None,
            path: Some(path.to_path_buf()),
            dir: Some(parent.to_path_buf()),
        });
    }
    rows.sort_by(|a, b| a.fork.cmp(&b.fork).then_with(|| a.file.cmp(&b.file)));
    rows
}

/// Assemble all rows: registered series first (registry order), then loose
/// patches.
pub fn collect(root: Option<&Path>, forks: &[Fork]) -> Vec<PatchRow> {
    let mut rows: Vec<PatchRow> = forks
        .iter()
        .flat_map(|fork| registry_rows(root, fork))
        .collect();
    if let Some(root) = root {
        rows.extend(loose_rows(root, forks));
    }
    rows
}

#[cfg(test)]
mod tests {
    use super::{Fork, parse_pr_url, patch_header, registry_rows};

    fn fork_with_two_entries() -> Fork {
        serde_json::from_value(serde_json::json!({
            "name": "demo",
            "patchDir": "series",
            "patches": {
                "a.patch": { "upstream": "attempt" },
                "removed.patch": { "upstream": "hold" },
            },
        }))
        .expect("fork")
    }

    #[test]
    fn checkout_series_dir_is_source_of_truth() {
        let root =
            std::env::temp_dir().join(format!("prs-discover-registry-test-{}", std::process::id()));
        std::fs::create_dir_all(root.join("series")).expect("mkdir");
        std::fs::write(root.join("series/a.patch"), "Subject: x\n").expect("write");
        let rows = registry_rows(Some(&root), &fork_with_two_entries());
        // The stale `removed.patch` mapping entry must not render a phantom row.
        assert_eq!(
            rows.iter().map(|row| row.file.as_str()).collect::<Vec<_>>(),
            ["a.patch"],
        );
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn mapping_keys_cover_the_no_checkout_case() {
        let rows = registry_rows(None, &fork_with_two_entries());
        assert_eq!(
            rows.iter().map(|row| row.file.as_str()).collect::<Vec<_>>(),
            ["a.patch", "removed.patch"],
        );
    }

    #[test]
    fn format_patch_header_stops_at_first_diff() {
        let text = "Subject: fix\n\nSee https://github.com/o/r/pull/1\n\
                    \ndiff --git a/x b/x\n+https://github.com/o/r/pull/2\n";
        assert!(patch_header(text).contains("pull/1"));
        assert!(!patch_header(text).contains("pull/2"));
    }

    #[test]
    fn raw_diff_has_no_header() {
        // A `git diff`-generated patch starts at the diff boundary; a PR URL
        // inside the diff body is not this patch's upstream PR.
        let text = "diff --git a/x b/x\n--- a/x\n+++ b/x\n\
                    +see https://github.com/o/r/pull/3\n";
        assert_eq!(patch_header(text), "");
    }

    #[test]
    fn parses_pr_urls() {
        let pr = parse_pr_url("see https://github.com/nushell/nushell/pull/18549 for status")
            .expect("pr");
        assert_eq!(pr.owner, "nushell");
        assert_eq!(pr.repo, "nushell");
        assert_eq!(pr.number, 18549);
        assert_eq!(pr.url, "https://github.com/nushell/nushell/pull/18549");
    }

    #[test]
    fn rejects_non_pr_urls() {
        assert!(parse_pr_url("https://github.com/NixOS/nix/issues/15962").is_none());
        assert!(
            parse_pr_url("https://gitlab.freedesktop.org/mesa/mesa/-/merge_requests/1").is_none()
        );
    }
}
