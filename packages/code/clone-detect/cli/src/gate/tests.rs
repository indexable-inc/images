use std::{
    collections::{BTreeMap, BTreeSet},
    path::PathBuf,
};

use clone_detect::{
    ByteRange, CloneGroup, DetectionResult, DetectionStats, Fragment, Kind, LineRange,
};

use super::{DiffGate, GateReport, GlobalGate};
use crate::diff::ChangedLines;

/// A detection result carrying `duplication_pct` and the given clone groups.
/// The non-fragment stats fields are irrelevant to gate math, so they are
/// zeroed except `duplication_pct`.
fn result(duplication_pct: f64, groups: Vec<CloneGroup>) -> DetectionResult {
    DetectionResult {
        instances: groups,
        stats: DetectionStats {
            files_scanned: 0,
            nodes_analyzed: 0,
            total_lines: 0,
            duplicated_lines: 0,
            duplication_pct,
            type1_groups: 0,
            type2_groups: 0,
            type3_groups: 0,
            sequence_groups: 0,
        },
    }
}

/// A Type-1 clone group with one fragment covering `file` rows `start..=end`.
/// Rows are tree-sitter's 0-indexed coordinate (as `Fragment::lines` carries);
/// the gate converts them to git's 1-indexed lines when comparing, so a
/// fragment at rows `start..=end` covers 1-indexed lines `start+1..=end+1`.
/// A group needs 2+ fragments in production, but gate math only reads the
/// covered line ranges, so a single fragment is enough to exercise coverage.
fn group(file: &str, start: usize, end: usize) -> CloneGroup {
    CloneGroup {
        clone_type: Kind::Type1,
        fragments: vec![Fragment {
            file: PathBuf::from(file),
            byte_range: ByteRange { start: 0, end: 0 },
            lines: LineRange { start, end },
            kind: "function_item".to_owned(),
        }],
    }
}

/// A `ChangedLines` from `(path, [lines])` pairs.
fn changed(entries: &[(&str, &[usize])]) -> ChangedLines {
    let mut map: BTreeMap<PathBuf, BTreeSet<usize>> = BTreeMap::new();
    for (path, lines) in entries {
        map.insert(PathBuf::from(*path), lines.iter().copied().collect());
    }
    ChangedLines(map)
}

#[test]
fn global_gate_passes_at_or_below_budget() {
    let r = result(1.05, vec![]);
    assert!(GlobalGate::evaluate(&r, 1.1).pass);
    // Exactly equal passes (metric <= budget).
    assert!(GlobalGate::evaluate(&r, 1.05).pass);
    // Above budget fails.
    assert!(!GlobalGate::evaluate(&r, 1.0).pass);
}

#[test]
#[expect(
    clippy::float_cmp,
    reason = "diff_pct is exactly 0.0 when there are no changed lines"
)]
fn diff_gate_zero_changed_lines_passes() {
    // No changed lines => diff_pct is 0, which passes even a 0.0 budget.
    let r = result(50.0, vec![group("a.rs", 1, 100)]);
    let g = DiffGate::evaluate(&r, &changed(&[]), 0.0, "origin/main".into(), "sha".into());
    assert_eq!(g.diff_pct, 0.0);
    assert_eq!(g.changed_lines, 0);
    assert_eq!(g.duplicated_changed_lines, 0);
    assert!(g.pass);
}

#[test]
#[expect(
    clippy::float_cmp,
    reason = "100.0 is exact when every changed line is duplicated"
)]
fn diff_gate_all_changed_lines_duplicated() {
    // A clone at rows 10..=20 covers 1-indexed lines 11..=21; the change touches
    // 12,13,14 — all inside it.
    let r = result(0.0, vec![group("a.rs", 10, 20)]);
    let g = DiffGate::evaluate(
        &r,
        &changed(&[("a.rs", &[12, 13, 14])]),
        50.0,
        "b".into(),
        "s".into(),
    );
    assert_eq!(g.changed_lines, 3);
    assert_eq!(g.duplicated_changed_lines, 3);
    assert_eq!(g.diff_pct, 100.0);
    // 100% > 50% budget => fail.
    assert!(!g.pass);
}

#[test]
fn diff_gate_partial_overlap() {
    // Clone at rows 10..=15 covers 1-indexed lines 11..=16. Changed lines are
    // 11,12,16,17,30: 11, 12, and 16 fall inside the clone, so 3 of 5 changed
    // lines are duplicated => 60%.
    let r = result(0.0, vec![group("a.rs", 10, 15)]);
    let g = DiffGate::evaluate(
        &r,
        &changed(&[("a.rs", &[11, 12, 16, 17, 30])]),
        60.0,
        "b".into(),
        "s".into(),
    );
    assert_eq!(g.changed_lines, 5);
    assert_eq!(g.duplicated_changed_lines, 3);
    assert!((g.diff_pct - 60.0).abs() < 1e-9);
    // Exactly at budget passes.
    assert!(g.pass);
}

#[test]
#[expect(
    clippy::float_cmp,
    reason = "diff_pct is exactly 0.0 when no changed line is duplicated"
)]
fn diff_gate_ignores_clones_in_unchanged_files() {
    // The clone is in b.rs but the change touched a.rs: nothing duplicated.
    let r = result(0.0, vec![group("b.rs", 1, 100)]);
    let g = DiffGate::evaluate(
        &r,
        &changed(&[("a.rs", &[1, 2, 3])]),
        0.0,
        "b".into(),
        "s".into(),
    );
    assert_eq!(g.duplicated_changed_lines, 0);
    assert_eq!(g.diff_pct, 0.0);
    assert!(g.pass);
}

#[test]
fn report_passes_only_when_all_enabled_gates_pass() {
    let passing_global = GlobalGate {
        duplication_pct: 1.0,
        budget_pct: 2.0,
        pass: true,
    };
    let failing_diff = DiffGate {
        diff_pct: 90.0,
        budget_pct: 10.0,
        pass: false,
        base: "b".into(),
        base_sha: "s".into(),
        changed_lines: 10,
        duplicated_changed_lines: 9,
    };

    // No gates: nothing to fail.
    assert!(
        GateReport {
            global: None,
            diff: None,
        }
        .passed()
    );
    // One gate passes.
    assert!(
        GateReport {
            global: Some(passing_global.clone()),
            diff: None,
        }
        .passed()
    );
    // A failing diff gate sinks the report even when global passes.
    assert!(
        !GateReport {
            global: Some(passing_global),
            diff: Some(failing_diff),
        }
        .passed()
    );
}
