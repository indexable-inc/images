//! Row model shared by discovery ([`crate::discover`]), the GitHub fetch
//! ([`crate::github`]), and both renderers.

use std::path::PathBuf;

/// A GitHub pull request reference parsed out of a URL.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct PrRef {
    pub owner: String,
    pub repo: String,
    pub number: u64,
    pub url: String,
}

impl PrRef {
    /// Compact `owner/repo#number` form for table cells.
    pub fn short(&self) -> String {
        let Self {
            owner,
            repo,
            number,
            ..
        } = self;
        format!("{owner}/{repo}#{number}")
    }
}

/// Where a patch's PR association was found, most authoritative first.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PrSource {
    /// Explicit `pr` field on the patch's entry in `lib/fork-packages.nix`.
    Mapping,
    /// Tracked by `upstream-sync` in the series' `upstream-status.json`.
    Status,
    /// A `https://github.com/<o>/<r>/pull/<n>` URL in the patch's own header.
    PatchHeader,
}

/// Live PR state as GraphQL reports it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PrState {
    Open,
    Draft,
    Merged,
    Closed,
}

// clone:ignore -- the repo-idiomatic const label() enum-to-str match; it only
// resembles unrelated enums (gmail's MessageFormat, subagent-cache's Outcome).
impl PrState {
    pub const fn label(self) -> &'static str {
        match self {
            Self::Open => "open",
            Self::Draft => "draft",
            Self::Merged => "merged",
            Self::Closed => "closed",
        }
    }
}

/// Combined status-check rollup for the PR's head commit.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CiState {
    Passing,
    Failing,
    Pending,
}

// clone:ignore -- same idiomatic label() shape as PrState above; three tiny
// enums each mapping variants to display words, not shared logic to extract.
impl CiState {
    pub const fn label(self) -> &'static str {
        match self {
            Self::Passing => "pass",
            Self::Failing => "fail",
            Self::Pending => "pending",
        }
    }
}

/// GraphQL `reviewDecision`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReviewState {
    Approved,
    ChangesRequested,
    ReviewRequired,
}

// clone:ignore -- same idiomatic label() shape as PrState above.
impl ReviewState {
    pub const fn label(self) -> &'static str {
        match self {
            Self::Approved => "approved",
            Self::ChangesRequested => "changes",
            Self::ReviewRequired => "required",
        }
    }
}

/// Everything the GitHub API tells us about one PR.
#[derive(Debug, Clone)]
pub struct PrStatus {
    pub state: PrState,
    pub ci: Option<CiState>,
    pub review: Option<ReviewState>,
    pub unresolved: usize,
    /// The review-thread page was full, so `unresolved` is a lower bound.
    pub unresolved_truncated: bool,
}

/// One patch file: the unit both renderers show one row for.
#[derive(Debug, Clone)]
pub struct PatchRow {
    /// Fork name from the registry, or the containing package's repo-relative
    /// path for loose patches found outside any registered series.
    pub fork: String,
    /// Patch file name (the stable identity the series and dag.json share).
    pub file: String,
    /// Declared upstream intent (`attempt` / `hold` / `never`); `None` for
    /// loose patches, which carry no registry entry.
    pub intent: Option<String>,
    pub pr: Option<PrRef>,
    pub pr_source: Option<PrSource>,
    pub status: Option<PrStatus>,
    /// The patch file on disk, for the TUI's edit (`e`) and diff-preview
    /// (`d`) keys. `None` when the row exists only as a mapping key (baked
    /// mapping, run outside a checkout).
    pub path: Option<PathBuf>,
    /// The directory holding the patch: the fork's series directory, or a
    /// loose patch's parent. For the TUI's edit-directory (`E`) key.
    pub dir: Option<PathBuf>,
}

impl PatchRow {
    /// Case-insensitive match against the fork, file, and PR columns, for the
    /// TUI's `/` filter.
    pub fn matches(&self, needle: &str) -> bool {
        let needle = needle.to_lowercase();
        self.fork.to_lowercase().contains(&needle)
            || self.file.to_lowercase().contains(&needle)
            || self
                .pr
                .as_ref()
                .is_some_and(|pr| pr.short().to_lowercase().contains(&needle))
    }
}
