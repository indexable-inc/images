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

/// One de-forked package from the mapping.
///
/// The fork lives in a real GitHub fork repo (`fork_repo`) whose
/// `bookmark` points at the megamerge commit; the patch series is that
/// commit's ancestry down to the upstream base. Unknown fields
/// (autoUpdate, derivedPatches, ...) belong to other tools and are
/// ignored here.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Fork {
    pub name: String,
    /// The flake input pinning the megamerge, for a fork fetched by rev.
    /// `None` for a vendored fork. Read it through [`Fork::source`] rather
    /// than directly: the invariant is the PAIR, and a caller that reaches for
    /// one field alone is the caller that forgets a vendored fork has no lock
    /// entry to look up.
    #[serde(default)]
    pub input: Option<String>,
    /// Repo-relative path of the in-tree derived view, for a fork carried in
    /// this repository rather than fetched. `None` for a rev-pinned fork.
    #[serde(default)]
    pub vendored: Option<String>,
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
    /// Whether the fork-sync cron floats this input onto each rebase. A
    /// floating input is legitimately off its bookmark between the rebase and
    /// the rolling PR merging, which is the whole reason [`crate::pin`] gates
    /// the two cases differently. Defaults to false, the stricter side, so an
    /// entry that forgets the flag is gated rather than exempt.
    #[serde(default)]
    pub auto_update: bool,
    /// A recorded, deliberate exception to the pin-on-bookmark rule.
    #[serde(default)]
    pub pin_divergence: Option<PinDivergence>,
    /// Per-patch intent keyed by the patch commit's SUBJECT line (the
    /// identity that survives jj rebases).
    #[serde(default)]
    pub patches: BTreeMap<String, PatchIntent>,
    pub upstream_policy: Option<Policy>,
}

fn default_bookmark() -> String {
    "ix-patched".to_owned()
}

/// Where a fork's tree comes from.
///
/// The two cases are exclusive and [`Fork::source`] is the only way to read
/// them, so nothing downstream can treat a vendored fork as a pinned one whose
/// lock entry happens to be missing. That distinction is the whole point: a
/// missing lock entry is a broken registry and FAILS the pin gate, while a
/// vendored fork has no pin by construction and must not.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Source<'a> {
    /// Fetched by rev: the named flake input pins the megamerge commit.
    Pinned(&'a str),
    /// Carried in this repository at this path, as a jj-views derived view of
    /// the fork repo. There is no rev, so there is nothing to drift from one.
    Vendored(&'a str),
}

impl Fork {
    /// Which of the two source shapes this entry declares.
    ///
    /// # Errors
    /// Fails when the entry declares both or neither. Neither default is safe:
    /// defaulting to pinned invents a lock lookup that cannot succeed, and
    /// defaulting to vendored exempts the entry from the pin gate, which is the
    /// silent-green failure [`crate::pin`] exists to refuse.
    pub fn source(&self) -> Result<Source<'_>> {
        match (self.input.as_deref(), self.vendored.as_deref()) {
            (Some(input), None) => Ok(Source::Pinned(input)),
            (None, Some(path)) => Ok(Source::Vendored(path)),
            (None, None) => Err(eyre!(
                "upstream-sync: fork {}: declares neither `input` nor `vendored`, so there is no \
                 way to know where its tree comes from",
                self.name
            )),
            (Some(input), Some(path)) => Err(eyre!(
                "upstream-sync: fork {}: declares both `input` ({input}) and `vendored` ({path}). \
                 A fork is fetched by rev or carried in tree, never both; two sources means two \
                 trees and nothing says which one ships.",
                self.name
            )),
        }
    }

    /// How to name this fork's source in a report cell.
    ///
    /// # Errors
    /// Fails for the reasons [`Fork::source`] does.
    pub fn source_label(&self) -> Result<String> {
        Ok(match self.source()? {
            Source::Pinned(input) => input.to_owned(),
            Source::Vendored(path) => format!("vendored:{path}"),
        })
    }
}

/// A recorded, deliberate exception to the pin-on-bookmark rule
/// ([`crate::pin`]).
///
/// Keyed by `rev` and not by fork, because an exemption that outlives the thing
/// it excused is how a gate stops gating: the next pin would inherit a waiver
/// nobody re-examined. The gate fails on a waiver whose rev is no longer
/// pinned, and on one whose fork is no longer diverged, so the only way to keep
/// one is to keep meaning it.
#[derive(Debug, Clone, Deserialize)]
pub struct PinDivergence {
    /// The full pinned rev this waiver covers.
    pub rev: String,
    /// Why it is not fixed yet, and what fixing it would mean.
    pub reason: String,
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

/// The complete per-patch stance vocabulary.
///
/// A value outside this set is a typo, and a typo is dangerous in the quiet
/// direction: every gate in the tool asks `== "attempt"`, so `atempt` reads
/// as "do not send" and the patch silently stops being contributed while its
/// registry entry still says it should be. [`validate`] rejects the whole
/// mapping instead.
///
/// `never` and `rejected` both stop a patch from being sent, and they are
/// separate words because they answer different questions: `never` is our
/// judgement that the patch does not belong upstream, `rejected` is the
/// upstream's judgement, already delivered.
pub const STANCES: [&str; 4] = ["attempt", "hold", "never", "rejected"];

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
    #[serde(default)]
    pub auto_contribute: AutoContribute,
}

/// Whether the unattended lane may open a PR against this upstream.
///
/// Separate from `prs_welcome` and `ai_prs_allowed`.
///
/// Those say whether a PR is acceptable at all. This says whether it is
/// acceptable UNINVITED and UNWATCHED, which is a different question:
/// ghostty welcomes AI-assisted PRs and still auto-closes a first-time
/// contributor's until a maintainer vouches, so an unattended PR there
/// opens straight into a close.
///
/// Defaults to disabled. A fork nobody has reviewed never sends an uninvited
/// PR, and [`validate`] refuses a mapping that leaves the reason blank, so
/// the default cannot be reached by silence.
#[derive(Debug, Clone, Deserialize)]
pub struct AutoContribute {
    #[serde(default)]
    pub enabled: bool,
    #[serde(default)]
    pub reason: String,
}

impl Default for AutoContribute {
    fn default() -> Self {
        Self {
            enabled: false,
            reason: "no autoContribute declared in lib/fork-packages.nix".to_owned(),
        }
    }
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
            auto_contribute: AutoContribute::default(),
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

/// Check the whole registry, so a bad entry fails the run rather than
/// quietly changing what gets contributed.
///
/// Two classes, both of which fail silently without this:
///   - an unknown stance word reads as "not attempt" everywhere, so a typo
///     retires a patch from contribution while its entry still claims
///     otherwise;
///   - an `autoContribute` with no reason leaves the stance unexplained, and
///     an unexplained gate is one the next person deletes or flips without
///     knowing what it defended.
///   - a `pinDivergence` waiver with no rev cannot expire, and one with no
///     reason is indistinguishable from an oversight the day someone reads it.
///
/// # Errors
/// Fails listing every problem found, not just the first: a registry edit
/// usually breaks several entries the same way.
pub fn validate(forks: &[Fork]) -> Result<()> {
    let mut problems: Vec<String> = Vec::new();
    for fork in forks {
        let policy = fork.policy();
        if policy.auto_contribute.reason.trim().is_empty() {
            problems.push(format!(
                "{}: upstreamPolicy.autoContribute has no reason; state why this repo is in or out of unattended contribution",
                fork.name
            ));
        }
        if let Some(waiver) = &fork.pin_divergence {
            // A 12-char rev is the tempting thing to paste from a table, and it
            // fails in the confusing direction: the waiver stops matching the pin
            // it was written for and the gate reports it as expired, naming the
            // rev that IS pinned.
            let rev = waiver.rev.trim();
            if rev.len() != 40 || !rev.chars().all(|c| c.is_ascii_hexdigit()) {
                problems.push(format!(
                    "{}: pinDivergence rev must be the full 40-character sha the lock pins, not {rev:?}; a waiver keyed on anything else cannot expire when the pin moves",
                    fork.name
                ));
            }
            if waiver.reason.trim().is_empty() {
                problems.push(format!(
                    "{}: pinDivergence states no reason; say why the pin is off the bookmark and what fixing it would mean",
                    fork.name
                ));
            }
        }
        for (subject, intent) in &fork.patches {

            let Some(stance) = intent.upstream.as_deref() else {
                continue;
            };
            if !STANCES.contains(&stance) {
                problems.push(format!(
                    "{}: patch '{subject}' has upstream = \"{stance}\", which is not one of {}",
                    fork.name,
                    STANCES.join(" | ")
                ));
            }
            if stance == "rejected" && intent.reason.as_deref().unwrap_or("").trim().is_empty() {
                problems.push(format!(
                    "{}: patch '{subject}' is rejected but states no reason; record what upstream said and where",
                    fork.name
                ));
            }
        }
    }
    if problems.is_empty() {
        return Ok(());
    }
    Err(eyre!(
        "lib/fork-packages.nix has {} problem(s):\n  - {}",
        problems.len(),
        problems.join("\n  - ")
    ))
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

    fn fork_with(patches: &str, policy: &str) -> Fork {
        serde_json::from_str(&format!(
            r#"{{"name":"fake","input":"fake-src","forkRepo":"indexable-inc/fakerepo",
                "upstreamUrl":"https://github.com/fakeorg/fakerepo.git",
                "patches":{patches},"upstreamPolicy":{policy}}}"#
        ))
        .unwrap()
    }

    fn fork_json(fields: &str) -> Fork {
        serde_json::from_str(&format!(
            r#"{{"name":"fake",{fields},"forkRepo":"indexable-inc/fakerepo",
                "upstreamUrl":"https://github.com/fakeorg/fakerepo.git",
                "upstreamPolicy":null}}"#
        ))
        .unwrap()
    }

    #[test]
    fn source_reads_a_rev_pinned_fork_as_pinned() {
        let fork = fork_json(r#""input":"fake-src""#);
        assert_eq!(fork.source().unwrap(), Source::Pinned("fake-src"));
        assert_eq!(fork.source_label().unwrap(), "fake-src");
    }

    #[test]
    fn source_reads_a_vendored_fork_as_vendored() {
        let fork = fork_json(r#""vendored":"vendor/fake""#);
        assert_eq!(fork.source().unwrap(), Source::Vendored("vendor/fake"));
        assert_eq!(fork.source_label().unwrap(), "vendored:vendor/fake");
    }

    // The two failures below are the point of the enum. Either default would be
    // silently wrong in a different direction, so both have to be refused, and
    // the message has to name which fork so the reader is not left grepping a
    // fourteen-entry registry.
    #[test]
    fn source_refuses_an_entry_declaring_neither() {
        let fork = fork_json(r#""bookmark":"ix-patched""#);
        let err = fork.source().unwrap_err().to_string();
        assert!(err.contains("fake"), "must name the fork, said: {err}");
        assert!(
            err.contains("neither"),
            "must say what is missing, said: {err}"
        );
    }

    #[test]
    fn source_refuses_an_entry_declaring_both() {
        let fork = fork_json(r#""input":"fake-src","vendored":"vendor/fake""#);
        let err = fork.source().unwrap_err().to_string();
        assert!(err.contains("fake-src"), "must name the input, said: {err}");
        assert!(
            err.contains("vendor/fake"),
            "must name the path, said: {err}"
        );
    }

    const GOOD_POLICY: &str =
        r#"{"autoContribute":{"enabled":false,"reason":"out: they close unsolicited PRs"}}"#;

    #[test]
    fn auto_contribute_defaults_off_and_says_it_was_never_declared() {
        let fork = fork_with("{}", "{}");
        let auto = fork.policy().auto_contribute;
        assert!(!auto.enabled);
        assert!(auto.reason.contains("no autoContribute declared"));
    }

    #[test]
    fn a_mapping_that_states_its_stances_and_reasons_passes() {
        let fork = fork_with(
            r#"{"a: keep":{"upstream":"never","reason":"repo-specific"},
                "b: turned down":{"upstream":"rejected","reason":"upstream closed it as out of scope in #12"}}"#,
            GOOD_POLICY,
        );
        validate(&[fork]).unwrap();
    }

    #[test]
    fn an_unexplained_gate_and_a_misspelled_stance_both_fail_together() {
        // Both problems in one mapping: validate reports every one, because
        // a registry edit usually breaks several entries the same way.
        let fork = fork_with(
            r#"{"a: typo":{"upstream":"atempt","reason":"r"}}"#,
            r#"{"autoContribute":{"enabled":true,"reason":"  "}}"#,
        );
        let err = validate(&[fork]).unwrap_err().to_string();
        assert!(err.contains("2 problem(s)"), "{err}");
        assert!(err.contains("autoContribute has no reason"), "{err}");
        assert!(
            err.contains("not one of attempt | hold | never | rejected"),
            "{err}"
        );
    }

    #[test]
    fn a_rejection_must_record_what_upstream_said() {
        let fork = fork_with(r#"{"a: turned down":{"upstream":"rejected"}}"#, GOOD_POLICY);
        let err = validate(&[fork]).unwrap_err().to_string();
        assert!(err.contains("rejected but states no reason"), "{err}");
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
