//! The fork-package mapping: declarative data from `lib/fork-packages.nix`.
//!
//! Rendered to JSON by the Nix wrapper (`UPSTREAM_SYNC_FORK_PACKAGES`) or
//! supplied by a downstream repo via `--mapping`. One tool, parameterized by
//! data, never copied.

use std::collections::BTreeMap;
use std::env;
use std::fs;
use std::path::{Path, PathBuf};

use color_eyre::eyre::{Result, WrapErr, eyre};
use serde::Deserialize;

/// Environment variable the Nix wrapper points at the baked-in fork list.
pub const FORK_PACKAGES_ENV: &str = "UPSTREAM_SYNC_FORK_PACKAGES";

/// One de-forked package from the mapping. The fork lives in a real GitHub
/// fork repo (`fork_repo`) whose `bookmark` points at the megamerge commit;
/// the patch series is that commit's ancestry down to the upstream base.
/// Unknown fields (autoUpdate, derivedPatches, ...) belong to other tools
/// and are ignored here.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Fork {
    pub name: String,
    pub input: String,
    /// GitHub `owner/name` of the maintained fork repo.
    pub fork_repo: String,
    /// Fork-repo branch holding the megamerge commit.
    #[serde(default = "default_bookmark")]
    pub bookmark: String,
    /// The upstream git URL contributions target.
    pub upstream_url: String,
    /// The upstream branch the fork's base sits on (e.g. nix on
    /// `2.34-maintenance`). `None` means the upstream's default branch,
    /// discovered from its HEAD symref.
    #[serde(default)]
    pub upstream_ref: Option<String>,
    /// Per-patch intent keyed by the patch commit's SUBJECT line (the
    /// identity that survives jj rebases).
    #[serde(default)]
    pub patches: BTreeMap<String, PatchIntent>,
    pub upstream_policy: Option<Policy>,
}

fn default_bookmark() -> String {
    "ix-patched".to_owned()
}

/// Hand-written per-patch intent: `attempt` is the human gate that authorizes
/// the outward act.
#[derive(Debug, Clone, Deserialize)]
pub struct PatchIntent {
    pub upstream: Option<String>,
    pub reason: Option<String>,
    #[serde(rename = "prExtra")]
    pub pr_extra: Option<String>,
}

/// Per-repo contribution policy from the mapping.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Policy {
    #[serde(default = "default_true")]
    pub prs_welcome: bool,
    #[serde(default = "default_unknown")]
    pub ai_prs_allowed: String,
    #[serde(default)]
    pub citation: String,
    #[serde(default)]
    pub notes: String,
}

const fn default_true() -> bool {
    true
}

fn default_unknown() -> String {
    "unknown".to_owned()
}

impl Default for Policy {
    fn default() -> Self {
        Self {
            prs_welcome: true,
            ai_prs_allowed: default_unknown(),
            citation: String::new(),
            notes: String::new(),
        }
    }
}

impl Fork {
    /// The https clone/push URL of the maintained fork repo.
    #[must_use]
    pub fn fork_url(&self) -> String {
        format!("https://github.com/{}.git", self.fork_repo)
    }

    /// The owner half of `fork_repo` (`gh pr create --head` wants
    /// `<owner>:<branch>`).
    #[must_use]
    pub fn fork_owner(&self) -> &str {
        self.fork_repo
            .split_once('/')
            .map_or(self.fork_repo.as_str(), |(owner, _)| owner)
    }

    /// The repo policy, defaulted when the mapping carries none.
    #[must_use]
    pub fn policy(&self) -> Policy {
        self.upstream_policy.clone().unwrap_or_default()
    }

    /// The declared stance of one patch (by commit subject). Fail-safe
    /// default: an unclassified patch is `hold`, never sent.
    #[must_use]
    pub fn stance(&self, subject: &str) -> String {
        self.patches
            .get(subject)
            .and_then(|m| m.upstream.clone())
            .unwrap_or_else(|| "hold".to_owned())
    }

    /// The declared reason of one patch, with the unclassified default.
    #[must_use]
    pub fn reason(&self, subject: &str) -> String {
        self.patches
            .get(subject)
            .and_then(|m| m.reason.clone())
            .unwrap_or_else(|| "unclassified (no intent entry in lib/fork-packages.nix)".to_owned())
    }
}

/// The mapping to drive: the caller `--mapping` path (a downstream repo
/// pointing this one tool at its own list) else the baked-in list from the
/// wrapper environment.
///
/// # Errors
/// Fails when neither a `--mapping` override nor the wrapper env var names a
/// mapping file.
pub fn path(override_path: Option<&Path>) -> Result<PathBuf> {
    if let Some(p) = override_path {
        return Ok(p.to_path_buf());
    }
    env::var_os(FORK_PACKAGES_ENV).map(PathBuf::from).ok_or_else(|| {
        eyre!("no --mapping given and {FORK_PACKAGES_ENV} is unset; run via the Nix wrapper or pass --mapping")
    })
}

/// Load every fork record from the mapping file.
///
/// # Errors
/// Fails when the mapping file is unreadable or not the expected JSON shape.
pub fn load(mapping: &Path) -> Result<Vec<Fork>> {
    let raw = fs::read_to_string(mapping)
        .wrap_err_with(|| format!("cannot read fork mapping {}", mapping.display()))?;
    serde_json::from_str(&raw)
        .wrap_err_with(|| format!("cannot parse fork mapping {}", mapping.display()))
}

/// Resolve the selected fork records from an optional name.
///
/// # Errors
/// Fails when `name` matches no fork in the mapping.
pub fn select(forks: Vec<Fork>, name: Option<&str>, tool: &str) -> Result<Vec<Fork>> {
    let Some(name) = name else { return Ok(forks) };
    let known: Vec<String> = forks.iter().map(|f| f.name.clone()).collect();
    let hit: Vec<Fork> = forks.into_iter().filter(|f| f.name == name).collect();
    if hit.is_empty() {
        return Err(eyre!(
            "{tool}: no fork package named {name}; known: {}",
            known.join(", ")
        ));
    }
    Ok(hit)
}

/// Owner/repo slug from an upstream https git URL, e.g.
/// `https://github.com/openai/codex.git` -> (`openai`, `codex`).
#[derive(Debug, Clone)]
pub struct Slug {
    pub owner: String,
    pub repo: String,
}

impl Slug {
    /// Parse the slug out of an upstream URL.
    ///
    /// # Errors
    /// Fails when the URL has fewer than two path segments.
    pub fn parse(url: &str) -> Result<Self> {
        let trimmed = url
            .trim_end_matches('/')
            .trim_end_matches(".git")
            .trim_end_matches('/');
        let parts: Vec<&str> = trimmed.split('/').collect();
        let [.., owner, repo] = parts.as_slice() else {
            return Err(eyre!("cannot parse owner/repo from upstream URL {url}"));
        };
        Ok(Self {
            owner: (*owner).to_owned(),
            repo: (*repo).to_owned(),
        })
    }
}

/// Is this upstream a GitHub repo? The gh-based PR + search path only works
/// for github.com; a non-github host (e.g. mesa on gitlab.freedesktop.org)
/// has no gh path, so we cannot track or open there.
#[must_use]
pub fn is_github(url: &str) -> bool {
    url.contains("github.com")
}

/// Is this upstream on a GitLab host (e.g. mesa on gitlab.freedesktop.org)?
#[must_use]
pub fn is_gitlab(url: &str) -> bool {
    url.contains("gitlab.")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn slug_strips_git_suffix_and_trailing_slash() {
        for (url, owner, repo) in [
            ("https://github.com/openai/codex.git", "openai", "codex"),
            ("https://gitlab.freedesktop.org/mesa/mesa/", "mesa", "mesa"),
        ] {
            let s = Slug::parse(url).unwrap();
            assert_eq!((s.owner.as_str(), s.repo.as_str()), (owner, repo), "{url}");
        }
    }

    #[test]
    fn forge_detection() {
        assert!(is_github("https://github.com/o/r.git"));
        assert!(!is_github("https://gitlab.freedesktop.org/mesa/mesa"));
        assert!(is_gitlab("https://gitlab.freedesktop.org/mesa/mesa"));
    }

    #[test]
    fn fork_defaults_bookmark_and_splits_owner() {
        let fork: Fork = serde_json::from_str(
            r#"{"name":"fake","input":"fake-src","forkRepo":"indexable-inc/fakerepo",
                "upstreamUrl":"https://github.com/fakeorg/fakerepo.git","patches":{}}"#,
        )
        .unwrap();
        assert_eq!(fork.bookmark, "ix-patched");
        assert_eq!(fork.upstream_ref, None);
        assert_eq!(fork.fork_owner(), "indexable-inc");
        assert_eq!(
            fork.fork_url(),
            "https://github.com/indexable-inc/fakerepo.git"
        );
        assert_eq!(fork.stance("anything"), "hold");
    }
}
