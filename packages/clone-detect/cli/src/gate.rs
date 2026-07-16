//! Budget gates over a `DetectionResult`.
//!
//! Two independent gates, each "metric must be <= budget":
//! - global: the whole-scan `duplication_pct`.
//! - diff: NEW duplication concentrated on the lines changed relative to a git
//!   base rev (see [`crate::diff`] for how the changed-line set is produced and
//!   [`crate::base`] for how pre-existing duplication is identified).
//!
//! The math here is pure: it takes an already-computed changed-line set, the
//! surviving clone fragments, and the base tree's fragments, and reports
//! pass/fail. All git/process work lives in [`crate::diff`] and [`crate::base`]
//! so this module stays testable without a repository.

use std::{
    collections::{BTreeMap, BTreeSet},
    path::{Path, PathBuf},
};

use clone_detect::DetectionResult;
use serde::Serialize;

use crate::diff::{ChangedLines, HunkOrigin};

/// The duplication that already existed at the diff base, in current-tree
/// coordinates (built by [`crate::base`]). Two independent kinds of evidence,
/// each surviving a different mechanical change (#3455):
///
/// - `counts`: per canonical file, how many surviving-group fragments carried
///   each AST fingerprint. Whitespace never reaches the AST, so a pure reflow
///   or an in-file move keeps a fragment's fingerprint; counting multiplicity
///   rather than mere membership keeps a NEW copy of an already-duplicated
///   shape chargeable even in the same file.
/// - `spans`: those fragments' line spans (1-indexed inclusive, git's
///   coordinate), keyed by the same files. Some reformats legitimately alter
///   the AST (rustfmt braces a closure body it splits across lines), which
///   breaks fingerprint identity; the changed lines still map back into an
///   already-duplicated base region through their hunk's old side
///   ([`HunkOrigin`]).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct BaseFragments {
    pub counts: BTreeMap<PathBuf, BTreeMap<u64, usize>>,
    pub spans: BTreeMap<PathBuf, Vec<(usize, usize)>>,
}

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

/// NEW duplication concentrated on changed lines.
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
    /// Of those, how many land on duplication the base did not already have:
    /// the new duplication the budget is charged for.
    pub duplicated_changed_lines: usize,
    /// Changed lines that land on duplicated code but are excused because the
    /// duplication is not new (#3455): every covering fragment's fingerprint
    /// multiplicity already existed at the base, or the line's hunk replaced a
    /// base region that was already inside a clone fragment (a reformat, an
    /// edit within an existing clone).
    pub preexisting_duplicated_changed_lines: usize,
    /// Exact duplicated changed lines, keyed by canonical source path.
    /// This makes a strict-gate failure actionable without a second scan.
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    pub duplicated_changed_line_locations: BTreeMap<PathBuf, BTreeSet<usize>>,
}

impl DiffGate {
    /// A changed line is charged as new duplication when a surviving clone
    /// fragment covers it and neither kind of base evidence excuses it:
    ///
    /// - fingerprint: every covering fragment's `(file, fingerprint)` count at
    ///   the base is at least its count now, i.e. this change did not add a
    ///   copy of that shape here (see [`BaseFragments::counts`]);
    /// - ancestry: the line's hunk replaced a base region overlapping a base
    ///   clone fragment, i.e. the code under it was already duplicated,
    ///   whatever the change did to its shape (see [`BaseFragments::spans`]).
    ///
    /// Pass an empty [`BaseFragments`] for the legacy
    /// every-covered-line-counts behavior. `diff_pct` is the ratio of charged
    /// lines to all changed lines; with no changed lines it is `0.0` (an empty
    /// diff cannot regress duplication), so it always passes any non-negative
    /// budget.
    #[must_use]
    pub fn evaluate(
        result: &DetectionResult,
        changed: &ChangedLines,
        budget_pct: f64,
        base: String,
        base_sha: String,
        preexisting: &BaseFragments,
    ) -> Self {
        let covered = covered_lines(result, preexisting);

        let mut changed_total = 0_usize;
        let mut preexisting_total = 0_usize;
        let mut duplicated_changed_line_locations: BTreeMap<PathBuf, BTreeSet<usize>> =
            BTreeMap::new();
        for (file, lines) in &changed.lines {
            changed_total += lines.len();
            let origins = changed.origins.get(file).map_or(&[][..], Vec::as_slice);
            let new_in_file = covered.new.get(file);
            let preexisting_in_file = covered.preexisting.get(file);
            let mut new_here = BTreeSet::new();
            for &line in lines {
                let on_new = new_in_file.is_some_and(|lines| lines.contains(&line));
                let on_preexisting = preexisting_in_file.is_some_and(|lines| lines.contains(&line));
                if !on_new && !on_preexisting {
                    continue;
                }
                // A line covered by both a new and a pre-existing fragment is
                // new duplication (something the base did not have now
                // duplicates it) unless its ancestry excuses it.
                if on_new && !ancestry_preexisting(origins, &preexisting.spans, line) {
                    new_here.insert(line);
                } else {
                    preexisting_total += 1;
                }
            }
            if !new_here.is_empty() {
                duplicated_changed_line_locations.insert(file.clone(), new_here);
            }
        }
        let duplicated = duplicated_changed_line_locations
            .values()
            .map(BTreeSet::len)
            .sum();

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
            preexisting_duplicated_changed_lines: preexisting_total,
            duplicated_changed_line_locations,
        }
    }
}

/// True when at least one changed line is covered by any surviving clone
/// fragment. The CLI uses this to decide whether the diff gate needs the
/// base-tree scan at all: with no overlap there is nothing to excuse, so the
/// (whole second) base scan is skipped.
#[must_use]
pub fn changed_lines_touch_clones(result: &DetectionResult, changed: &ChangedLines) -> bool {
    let covered = covered_lines(result, &BaseFragments::default()).new;
    changed.lines.iter().any(|(file, lines)| {
        covered
            .get(file)
            .is_some_and(|covered_in_file| lines.intersection(covered_in_file).next().is_some())
    })
}

/// True when `line`'s hunk maps back to a base region overlapping a base
/// clone fragment: the code under this line was already duplicated at the
/// base, whatever the change did to its shape. A pure insertion (empty old
/// side) replaced nothing, so it has no ancestry and is never excused.
fn ancestry_preexisting(
    origins: &[HunkOrigin],
    spans: &BTreeMap<PathBuf, Vec<(usize, usize)>>,
    line: usize,
) -> bool {
    origins.iter().any(|origin| {
        line >= origin.new_start
            && line < origin.new_start + origin.new_count
            && origin.old_count > 0
            && spans.get(&origin.old_path).is_some_and(|spans| {
                let old_end = origin.old_start + origin.old_count - 1;
                spans
                    .iter()
                    .any(|&(start, end)| start <= old_end && origin.old_start <= end)
            })
    })
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

/// Source lines covered by surviving clone fragments, keyed by file and in
/// git's 1-indexed coordinate, split by whether the covering fragment embodies
/// duplication the diff base already had.
struct Coverage {
    /// Lines covered by at least one fragment whose duplication grew here.
    new: BTreeMap<PathBuf, BTreeSet<usize>>,
    /// Lines covered only by fragments whose duplication the base already had.
    preexisting: BTreeMap<PathBuf, BTreeSet<usize>>,
}

/// Canonicalize a fragment path so it matches the absolute keys `ChangedLines`
/// and [`BaseFragments`] use, however the scan target was spelled.
fn canonical(path: &Path) -> PathBuf {
    std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf())
}

/// Compute [`Coverage`] over every surviving clone fragment. A fragment is
/// pre-existing when the base had at least as many fragments with its
/// fingerprint in its file as the head does: the duplication it embodies did
/// not grow in this change.
///
/// `Fragment::lines` comes from tree-sitter `Node::start_position().row`, which
/// is 0-indexed; `ChangedLines` comes from `git diff`, which is 1-indexed. We
/// convert the fragment ranges here (`+1`) so both sides compare in the same
/// coordinate. Comparing raw would shift every fragment up by one line and
/// mis-attribute duplication.
fn covered_lines(result: &DetectionResult, preexisting: &BaseFragments) -> Coverage {
    // Distinct fragments per (file, fingerprint): one fragment can sit in
    // several groups (a Type-1 pair is also a Type-2 pair), which must not
    // inflate the head-side multiplicity.
    let mut distinct: BTreeMap<PathBuf, BTreeSet<(usize, u64)>> = BTreeMap::new();
    for group in &result.instances {
        for fragment in &group.fragments {
            distinct
                .entry(canonical(&fragment.file))
                .or_default()
                .insert((fragment.byte_range.start, fragment.fingerprint));
        }
    }
    let head_counts: BTreeMap<&PathBuf, BTreeMap<u64, usize>> = distinct
        .iter()
        .map(|(file, fragments)| {
            let mut counts: BTreeMap<u64, usize> = BTreeMap::new();
            for &(_, fingerprint) in fragments {
                *counts.entry(fingerprint).or_default() += 1;
            }
            (file, counts)
        })
        .collect();

    let mut coverage = Coverage {
        new: BTreeMap::new(),
        preexisting: BTreeMap::new(),
    };
    for group in &result.instances {
        for fragment in &group.fragments {
            // Canonicalize so fragment paths (spelled however the scan target
            // was) match the absolute keys `ChangedLines` and the base
            // fragments use.
            let key = canonical(&fragment.file);
            let head = head_counts
                .get(&key)
                .and_then(|counts| counts.get(&fragment.fingerprint))
                .copied()
                .expect("every fragment was counted above");
            let base = preexisting
                .counts
                .get(&key)
                .and_then(|counts| counts.get(&fragment.fingerprint))
                .copied()
                .unwrap_or(0);
            let side = if base >= head {
                &mut coverage.preexisting
            } else {
                &mut coverage.new
            };
            let entry = side.entry(key).or_default();
            for row in fragment.lines.start..=fragment.lines.end {
                entry.insert(row + 1);
            }
        }
    }
    coverage
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
