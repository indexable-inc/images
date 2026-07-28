//! The JSON contract printed on stdout.

use std::path::PathBuf;

use complexity_metric::Unit;

/// One measured unit, located.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Located {
    pub file: PathBuf,
    pub language: String,
    #[serde(flatten)]
    pub unit: Unit,
    /// True when `cognitive` is at or above this language's threshold.
    pub over_threshold: bool,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Stats {
    pub files_scanned: usize,
    /// Files whose language has a profile. The rest are counted but not
    /// measured, so a growing uncovered corpus is visible rather than silent.
    pub files_measured: usize,
    pub units: usize,
    pub over_threshold: usize,
    /// Sum of cognitive complexity over every measured unit. A trend line
    /// only: it grows with the codebase, so it is not gated.
    pub total_cognitive: u64,
}

/// The gate's verdict, present only when a budget is configured.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Gate {
    pub over_threshold: usize,
    pub budget: usize,
    pub pass: bool,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Report {
    /// The worst units, truncated to `--top`. `stats` always counts them all.
    pub units: Vec<Located>,
    pub stats: Stats,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub budget: Option<Gate>,
}

/// Passing at exactly the budget keeps the ratchet's meaning: the budget is
/// the number of allowed violations, not the first forbidden count.
#[must_use]
pub const fn passes(over: usize, budget: usize) -> bool {
    over <= budget
}

#[cfg(test)]
mod tests {
    use super::passes;

    #[test]
    fn the_budget_is_the_last_allowed_count_not_the_first_forbidden_one() {
        assert!(passes(6, 7));
        assert!(passes(7, 7));
        assert!(!passes(8, 7));
    }
}
