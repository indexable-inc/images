//! Budget gates over a `DetectionResult`.
//!
//! Two independent gates, each "metric must be <= budget":
//! - global: the whole-scan `duplication_pct`.
//! - diff: duplication concentrated on the lines changed relative to a git base
//!   rev (see [`crate::diff`] for how the changed-line set is produced).
//!
//! The math here is pure: it takes an already-computed changed-line set and the
//! surviving clone fragments and reports pass/fail. All git/process work lives
//! in [`crate::diff`] so this module stays testable without a repository.

use std::{
    collections::{BTreeMap, BTreeSet},
    path::PathBuf,
};

use clone_detect::DetectionResult;
use serde::Serialize;

use crate::diff::ChangedLines;

/// A gate metric is duplicated when it lies at or below its budget. Fail is the
/// strict complement, so a metric exactly equal to the budget passes.
fn passes(metric: f64, budget: f64) -> bool {
    metric <= budget
}

/// Whole-scan duplication gate.
#[derive(Debug, Clone, Serialize)]
pub struct GlobalGate {
    /// `stats.duplication_pct` from the detection result.
    pub duplication_pct: f64,
    /// The configured ceiling.
    pub budget_pct: f64,
    pub pass: bool,
}

impl GlobalGate {
    #[must_use]
    pub fn evaluate(result: &DetectionResult, budget_pct: f64) -> Self {
        let duplication_pct = result.stats.duplication_pct;
        Self {
            duplication_pct,
            budget_pct,
            pass: passes(duplication_pct, budget_pct),
        }
    }
}

/// Duplication concentrated on changed lines.
#[derive(Debug, Clone, Serialize)]
pub struct DiffGate {
    /// `100 * duplicated_changed_lines / changed_lines`, or `0.0` when nothing
    /// changed.
    pub diff_pct: f64,
    /// The configured ceiling.
    pub budget_pct: f64,
    pub pass: bool,
    /// The base ref as requested (e.g. `origin/main`), before resolution.
    pub base: String,
    /// The merge-base commit the diff was taken against.
    pub base_sha: String,
    /// Total added/modified lines across all changed files.
    pub changed_lines: usize,
    /// Of those, how many a surviving clone fragment covers.
    pub duplicated_changed_lines: usize,
}

impl DiffGate {
    /// A changed line is "duplicated" when a surviving clone fragment in the
    /// same file covers it. `diff_pct` is the ratio of such lines to all
    /// changed lines; with no changed lines it is `0.0` (an empty diff cannot
    /// regress duplication), so it always passes any non-negative budget.
    #[must_use]
    pub fn evaluate(
        result: &DetectionResult,
        changed: &ChangedLines,
        budget_pct: f64,
        base: String,
        base_sha: String,
    ) -> Self {
        let covered = covered_lines(result);

        let mut changed_total = 0_usize;
        let mut duplicated = 0_usize;
        for (file, lines) in &changed.0 {
            changed_total += lines.len();
            if let Some(covered_in_file) = covered.get(file) {
                duplicated += lines.intersection(covered_in_file).count();
            }
        }

        // Ratio in percent; guard the zero-changed-lines case so an empty diff
        // reports 0% rather than NaN.
        let diff_pct = if changed_total == 0 {
            0.0
        } else {
            ratio_pct(duplicated, changed_total)
        };

        Self {
            diff_pct,
            budget_pct,
            pass: passes(diff_pct, budget_pct),
            base,
            base_sha,
            changed_lines: changed_total,
            duplicated_changed_lines: duplicated,
        }
    }
}

/// `100 * numerator / denominator` computed in f64. Callers guarantee
/// `denominator > 0`.
#[expect(
    clippy::cast_precision_loss,
    reason = "line counts are far below f64's 2^53 exact-integer range"
)]
fn ratio_pct(numerator: usize, denominator: usize) -> f64 {
    100.0 * numerator as f64 / denominator as f64
}

/// The set of source lines covered by any surviving clone fragment, keyed by
/// file and in git's 1-indexed coordinate.
///
/// `Fragment::lines` comes from tree-sitter `Node::start_position().row`, which
/// is 0-indexed; `ChangedLines` comes from `git diff`, which is 1-indexed. We
/// convert the fragment ranges here (`+1`) so both sides compare in the same
/// coordinate. Comparing raw would shift every fragment up by one line and
/// mis-attribute duplication.
fn covered_lines(result: &DetectionResult) -> BTreeMap<PathBuf, BTreeSet<usize>> {
    let mut covered: BTreeMap<PathBuf, BTreeSet<usize>> = BTreeMap::new();
    for group in &result.instances {
        for fragment in &group.fragments {
            // Canonicalize so fragment paths (spelled however the scan target
            // was) match the absolute keys `ChangedLines` uses.
            let key = std::fs::canonicalize(&fragment.file).unwrap_or_else(|_| fragment.file.clone());
            let entry = covered.entry(key).or_default();
            for row in fragment.lines.start..=fragment.lines.end {
                entry.insert(row + 1);
            }
        }
    }
    covered
}

/// The overall gate outcome for the enabled gates, serialized under the `gate`
/// key of the CLI's JSON output. A gate is `None` when it was not enabled.
#[derive(Debug, Clone, Serialize)]
pub struct GateReport {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub global: Option<GlobalGate>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub diff: Option<DiffGate>,
}

impl GateReport {
    /// True when every enabled gate passed. With no gate enabled there is
    /// nothing to fail, so it passes (the caller decides whether that is legal;
    /// legacy "any clone fails" is modeled as a global budget of `0.0`).
    #[must_use]
    pub fn passed(&self) -> bool {
        self.global.as_ref().is_none_or(|g| g.pass) && self.diff.as_ref().is_none_or(|d| d.pass)
    }
}

#[cfg(test)]
mod tests;
