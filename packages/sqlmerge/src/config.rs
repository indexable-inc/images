//! Declarative per-table conflict policy: the *data* half of the design.
//!
//! This module is pure: it parses `sqlmerge.toml` into a [`PolicyConfig`] and
//! answers "which policy applies to table T?" ([`PolicyConfig::policy_for`]).
//! It knows nothing about `SQLite`, changesets, or the apply path; the merge
//! engine (machinery) consults a [`PolicyConfig`] per conflict. Keeping the
//! policy table separate from the apply logic is deliberate: one source of
//! truth for the mapping, no policy branches scattered through the C callback.
//!
//! The on-disk format is a single `[policies]` table mapping a table-name GLOB
//! to a policy name:
//!
//! ```toml
//! [policies]
//! "cache_*" = "theirs"
//! "events"  = "append-only"
//! "*"       = "abort"
//! ```
//!
//! Precedence when several globs match one table is **first-listed wins**, in
//! the order the entries appear in the file. An absent file (or absent
//! `[policies]` table) means every table uses [`ConflictPolicy::Abort`], which
//! is exactly the pre-config behavior.

use std::fs;
use std::path::{Path, PathBuf};

use std::collections::BTreeMap;

use glob::{Pattern, PatternError};
use serde::Deserialize;
use toml::Spanned;

use crate::merge::ConflictPolicy;

/// Failures loading or parsing `sqlmerge.toml`.
///
/// A malformed config is a loud refusal, never a silent fall-back to abort: a
/// driver that ignored a misconfigured policy file would resolve conflicts the
/// operator did not ask for.
#[derive(Debug)]
pub enum ConfigError {
    /// The config file exists but could not be read.
    Read {
        path: PathBuf,
        source: std::io::Error,
    },
    /// The config file is not valid TOML or has an unknown policy name.
    Parse {
        path: PathBuf,
        source: toml::de::Error,
    },
    /// A key in `[policies]` is not a valid glob pattern.
    BadGlob {
        pattern: String,
        source: PatternError,
    },
}

impl std::fmt::Display for ConfigError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Read { path, source } => {
                write!(f, "cannot read {}: {source}", path.display())
            }
            Self::Parse { path, source } => {
                write!(f, "invalid sqlmerge config {}: {source}", path.display())
            }
            Self::BadGlob { pattern, source } => {
                write!(f, "invalid table glob {pattern:?}: {source}")
            }
        }
    }
}

impl std::error::Error for ConfigError {}

/// The policy name as it appears in `sqlmerge.toml`. A separate serde-facing
/// enum (kebab-case, denies unknown names) keeps the wire format decoupled from
/// the engine's [`ConflictPolicy`]; `from` maps one to the other.
#[derive(Debug, Clone, Copy, Deserialize)]
#[serde(rename_all = "kebab-case")]
enum PolicyName {
    Abort,
    Ours,
    Theirs,
    AppendOnly,
}

impl From<PolicyName> for ConflictPolicy {
    fn from(name: PolicyName) -> Self {
        match name {
            PolicyName::Abort => Self::Abort,
            PolicyName::Ours => Self::Ours,
            PolicyName::Theirs => Self::Theirs,
            PolicyName::AppendOnly => Self::AppendOnly,
        }
    }
}

/// The raw `[policies]` table as read from TOML.
///
/// The key is the table-name glob. The value is [`Spanned`], so we recover each
/// entry's byte offset in the source and re-establish declaration order from
/// it: `toml::Table` iterates its keys sorted, not in written order, so
/// first-listed precedence would otherwise be lost. Spans are exact and need no
/// order-preserving map dependency.
#[derive(Debug, Deserialize)]
struct RawConfig {
    #[serde(default)]
    policies: BTreeMap<String, Spanned<PolicyName>>,
}

/// One compiled `(glob, policy)` rule.
#[derive(Debug, Clone)]
struct Rule {
    pattern: Pattern,
    policy: ConflictPolicy,
}

/// The parsed policy table: an ordered list of glob rules. Match a table name
/// against the rules in order; the first hit wins. No match falls back to
/// [`ConflictPolicy::Abort`].
#[derive(Debug, Clone, Default)]
pub struct PolicyConfig {
    rules: Vec<Rule>,
}

impl PolicyConfig {
    /// The empty config: every table aborts on conflict. This is the default
    /// when no `sqlmerge.toml` is present.
    #[must_use]
    pub fn abort_all() -> Self {
        Self::default()
    }

    /// The policy for `table`: the first rule whose glob matches, else
    /// [`ConflictPolicy::Abort`].
    #[must_use]
    pub fn policy_for(&self, table: &str) -> ConflictPolicy {
        self.rules
            .iter()
            .find(|rule| rule.pattern.matches(table))
            .map_or(ConflictPolicy::Abort, |rule| rule.policy)
    }

    /// Parse a `sqlmerge.toml` body into a [`PolicyConfig`], attributing any
    /// error to a synthetic in-memory path. Prefer [`PolicyConfig::load_from`]
    /// for the real on-disk load; this is the entry point for tests and callers
    /// that already hold the config text.
    ///
    /// # Errors
    ///
    /// Returns [`ConfigError::Parse`] if `body` is not valid TOML or names an
    /// unknown policy, or [`ConfigError::BadGlob`] if a key is not a valid glob.
    pub fn load_body(body: &str) -> Result<Self, ConfigError> {
        Self::parse(body, Path::new("<in-memory sqlmerge.toml>"))
    }

    /// Parse a `sqlmerge.toml` body into a [`PolicyConfig`].
    ///
    /// # Errors
    ///
    /// Returns [`ConfigError::Parse`] if `body` is not valid TOML or names an
    /// unknown policy, or [`ConfigError::BadGlob`] if a key is not a valid glob.
    fn parse(body: &str, path: &Path) -> Result<Self, ConfigError> {
        let raw: RawConfig = toml::from_str(body).map_err(|source| ConfigError::Parse {
            path: path.to_path_buf(),
            source,
        })?;

        // Re-establish declaration order from the value spans: the map iterates
        // sorted, but first-listed precedence follows the order written in the
        // file. Sorting the entries by span start recovers it exactly.
        let mut entries: Vec<(String, Spanned<PolicyName>)> = raw.policies.into_iter().collect();
        entries.sort_by_key(|(_, spanned)| spanned.span().start);

        let mut rules = Vec::with_capacity(entries.len());
        for (pattern_str, spanned) in entries {
            let pattern = Pattern::new(&pattern_str).map_err(|source| ConfigError::BadGlob {
                pattern: pattern_str.clone(),
                source,
            })?;
            rules.push(Rule {
                pattern,
                policy: (*spanned.get_ref()).into(),
            });
        }

        Ok(Self { rules })
    }

    /// Load the policy config for the working tree rooted at `start`: walk up
    /// from `start` looking for `sqlmerge.toml` (git runs a merge driver at the
    /// worktree root, but walking up is robust to being invoked elsewhere).
    /// Absent file -> [`PolicyConfig::abort_all`].
    ///
    /// # Errors
    ///
    /// Returns [`ConfigError`] if a `sqlmerge.toml` is found but cannot be read
    /// or parsed. A missing file is not an error.
    pub fn load_from(start: &Path) -> Result<Self, ConfigError> {
        let Some(path) = find_config(start) else {
            return Ok(Self::abort_all());
        };
        let body = fs::read_to_string(&path).map_err(|source| ConfigError::Read {
            path: path.clone(),
            source,
        })?;
        Self::parse(&body, &path)
    }
}

/// Walk up from `start` (a directory) to the filesystem root, returning the
/// first `sqlmerge.toml` found.
fn find_config(start: &Path) -> Option<PathBuf> {
    start.ancestors().find_map(|dir| {
        let candidate = dir.join("sqlmerge.toml");
        candidate.is_file().then_some(candidate)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(body: &str) -> PolicyConfig {
        PolicyConfig::parse(body, Path::new("sqlmerge.toml")).expect("valid config")
    }

    #[test]
    fn empty_config_aborts_every_table() {
        let cfg = PolicyConfig::abort_all();
        assert_eq!(cfg.policy_for("anything"), ConflictPolicy::Abort);
        assert_eq!(parse("").policy_for("t"), ConflictPolicy::Abort);
    }

    #[test]
    fn exact_table_name_matches() {
        let cfg = parse("[policies]\n\"users\" = \"ours\"\n");
        assert_eq!(cfg.policy_for("users"), ConflictPolicy::Ours);
        // Non-matching table falls back to abort.
        assert_eq!(cfg.policy_for("posts"), ConflictPolicy::Abort);
    }

    #[test]
    fn star_wildcard_matches_prefix() {
        let cfg = parse("[policies]\n\"cache_*\" = \"theirs\"\n");
        assert_eq!(cfg.policy_for("cache_users"), ConflictPolicy::Theirs);
        assert_eq!(cfg.policy_for("cache_"), ConflictPolicy::Theirs);
        assert_eq!(cfg.policy_for("users"), ConflictPolicy::Abort);
    }

    #[test]
    fn question_mark_matches_single_char() {
        let cfg = parse("[policies]\n\"log?\" = \"append-only\"\n");
        assert_eq!(cfg.policy_for("logs"), ConflictPolicy::AppendOnly);
        assert_eq!(cfg.policy_for("log1"), ConflictPolicy::AppendOnly);
        // `?` is exactly one char: neither zero nor two match.
        assert_eq!(cfg.policy_for("log"), ConflictPolicy::Abort);
        assert_eq!(cfg.policy_for("logss"), ConflictPolicy::Abort);
    }

    #[test]
    fn first_listed_wins_when_multiple_globs_match() {
        // Both patterns match "cache_hot"; the first listed (theirs) wins.
        let cfg = parse(
            "[policies]\n\"cache_*\" = \"theirs\"\n\"*\" = \"abort\"\n",
        );
        assert_eq!(cfg.policy_for("cache_hot"), ConflictPolicy::Theirs);
        // A table only the second pattern matches gets the second policy.
        assert_eq!(cfg.policy_for("users"), ConflictPolicy::Abort);
    }

    #[test]
    fn catch_all_star_sets_the_default() {
        let cfg = parse("[policies]\n\"*\" = \"ours\"\n");
        assert_eq!(cfg.policy_for("anything"), ConflictPolicy::Ours);
    }

    #[test]
    fn unknown_policy_name_is_a_parse_error() {
        let err = PolicyConfig::parse(
            "[policies]\n\"t\" = \"last-writer-wins\"\n",
            Path::new("sqlmerge.toml"),
        )
        .expect_err("unknown policy should fail");
        assert!(matches!(err, ConfigError::Parse { .. }), "got {err:?}");
    }

    #[test]
    fn invalid_glob_is_a_typed_error() {
        // An unclosed character class is an invalid glob.
        let err = PolicyConfig::parse(
            "[policies]\n\"t[\" = \"ours\"\n",
            Path::new("sqlmerge.toml"),
        )
        .expect_err("bad glob should fail");
        assert!(matches!(err, ConfigError::BadGlob { .. }), "got {err:?}");
    }
}
