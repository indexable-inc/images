use std::{
    collections::{BTreeMap, BTreeSet},
    path::PathBuf,
};

use clone_detect::{
    ByteRange, CloneGroup, DetectionResult, DetectionStats, Fragment, Kind, LineRange,
};

use super::{BaseFragments, DiffGate, GateReport, GlobalGate, changed_lines_touch_clones};
use crate::diff::{ChangedLines, HunkOrigin};

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

/// A Type-1 clone group with one fragment covering `file` rows `start..=end`,
/// carrying `fingerprint`.
/// Rows are tree-sitter's 0-indexed coordinate (as `Fragment::lines` carries);
/// the gate converts them to git's 1-indexed lines when comparing, so a
/// fragment at rows `start..=end` covers 1-indexed lines `start+1..=end+1`.
/// A group needs 2+ fragments in production, but gate math only reads the
/// covered line ranges, so a single fragment is enough to exercise coverage.
fn group_fp(file: &str, start: usize, end: usize, fingerprint: u64) -> CloneGroup {
    CloneGroup {
        clone_type: Kind::Type1,
        fragments: vec![Fragment {
            file: PathBuf::from(file),
            // Distinct per location: the gate dedupes a fragment appearing in
            // several groups by its byte start, so give each row span its own.
            byte_range: ByteRange { start, end },
            lines: LineRange { start, end },
            kind: "function_item".to_owned(),
            generated: false,
            fingerprint,
        }],
    }
}

/// [`group_fp`] for tests where the fingerprint is irrelevant.
fn group(file: &str, start: usize, end: usize) -> CloneGroup {
    group_fp(file, start, end, 0)
}

/// A `BaseFragments` with the given fingerprint multiplicity per path and no
/// spans.
fn base_counts(entries: &[(&str, u64, usize)]) -> BaseFragments {
    let mut base = BaseFragments::default();
    for &(path, fingerprint, count) in entries {
        base.counts
            .entry(PathBuf::from(path))
            .or_default()
            .insert(fingerprint, count);
    }
    base
}

/// A `ChangedLines` from `(path, [lines])` pairs, with no hunk ancestry.
fn changed(entries: &[(&str, &[usize])]) -> ChangedLines {
    let mut lines: BTreeMap<PathBuf, BTreeSet<usize>> = BTreeMap::new();
    for (path, changed_lines) in entries {
        lines.insert(
            PathBuf::from(*path),
            changed_lines.iter().copied().collect(),
        );
    }
    ChangedLines {
        lines,
        origins: BTreeMap::new(),
    }
}

/// [`DiffGate::evaluate`] at `budget_pct` with placeholder base refs.
fn eval(r: &DetectionResult, ch: &ChangedLines, budget_pct: f64, pre: &BaseFragments) -> DiffGate {
    DiffGate::evaluate(r, ch, budget_pct, "b".into(), "s".into(), pre)
}

/// A `BaseFragments` carrying only ancestry spans for `a.rs`.
fn base_spans(spans: &[(usize, usize)]) -> BaseFragments {
    let mut base = BaseFragments::default();
    base.spans.insert(PathBuf::from("a.rs"), spans.to_vec());
    base
}

/// [`changed`] for `a.rs` at `lines`, whose one hunk replaced the old-side
/// range `(old_start, old_count)` of `a.rs` with the new-side range
/// `(new_start, new_count)`.
fn changed_via_hunk(
    lines: &[usize],
    (new_start, new_count): (usize, usize),
    (old_start, old_count): (usize, usize),
) -> ChangedLines {
    let mut ch = changed(&[("a.rs", lines)]);
    ch.origins.insert(
        PathBuf::from("a.rs"),
        vec![HunkOrigin {
            new_start,
            new_count,
            old_path: PathBuf::from("a.rs"),
            old_start,
            old_count,
        }],
    );
    ch
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
    let g = eval(&r, &changed(&[]), 0.0, &BaseFragments::default());
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
    let g = eval(
        &r,
        &changed(&[("a.rs", &[12, 13, 14])]),
        50.0,
        &BaseFragments::default(),
    );
    assert_eq!(g.changed_lines, 3);
    assert_eq!(g.duplicated_changed_lines, 3);
    assert_eq!(
        g.duplicated_changed_line_locations[&PathBuf::from("a.rs")],
        BTreeSet::from([12, 13, 14])
    );
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
    let g = eval(
        &r,
        &changed(&[("a.rs", &[11, 12, 16, 17, 30])]),
        60.0,
        &BaseFragments::default(),
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
    let g = eval(
        &r,
        &changed(&[("a.rs", &[1, 2, 3])]),
        0.0,
        &BaseFragments::default(),
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
        preexisting_duplicated_changed_lines: 0,
        duplicated_changed_line_locations: BTreeMap::new(),
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

/// Fingerprint-multiplicity cases (#3455): whether the duplication under a
/// changed line already existed at the base is decided per file by comparing
/// each fingerprint's fragment count now against the base's. A reflow moves a
/// fragment's line span, never its AST fingerprint, so pre-existing copies
/// survive reformats; adding a copy raises the count and stays chargeable.
#[test]
fn diff_gate_fingerprint_multiplicity() {
    struct Case {
        why: &'static str,
        groups: Vec<CloneGroup>,
        base: BaseFragments,
        lines: &'static [usize],
        charged: usize,
        excused: usize,
    }
    let cases = [
        Case {
            why: "a fragment whose (file, fingerprint) count the base already had is excused",
            groups: vec![group_fp("a.rs", 10, 20, 7)],
            base: base_counts(&[("a.rs", 7, 1)]),
            lines: &[12, 13],
            charged: 0,
            excused: 2,
        },
        Case {
            why: "a line also covered by a NEW fragment is new duplication",
            groups: vec![group_fp("a.rs", 10, 20, 7), group_fp("a.rs", 10, 20, 9)],
            base: base_counts(&[("a.rs", 7, 1)]),
            lines: &[12],
            charged: 1,
            excused: 0,
        },
        Case {
            why: "the fingerprint pre-existing in ANOTHER file does not excuse a fresh copy here",
            groups: vec![group_fp("a.rs", 10, 20, 7)],
            base: base_counts(&[("b.rs", 7, 1)]),
            lines: &[12],
            charged: 1,
            excused: 0,
        },
        Case {
            why: "an added same-file copy raises the multiplicity past the base's",
            groups: vec![group_fp("a.rs", 10, 20, 7), group_fp("a.rs", 30, 40, 7)],
            base: base_counts(&[("a.rs", 7, 1)]),
            lines: &[32],
            charged: 1,
            excused: 0,
        },
        Case {
            why: "one fragment in several groups is one copy: membership must not inflate multiplicity",
            groups: vec![group_fp("a.rs", 10, 20, 7), group_fp("a.rs", 10, 20, 7)],
            base: base_counts(&[("a.rs", 7, 1)]),
            lines: &[12],
            charged: 0,
            excused: 1,
        },
    ];
    for case in cases {
        let r = result(0.0, case.groups);
        let g = eval(&r, &changed(&[("a.rs", case.lines)]), 0.0, &case.base);
        assert_eq!(g.duplicated_changed_lines, case.charged, "{}", case.why);
        assert_eq!(
            g.preexisting_duplicated_changed_lines, case.excused,
            "{}",
            case.why
        );
        assert_eq!(g.pass, case.charged == 0, "{}", case.why);
    }
}

/// Hunk-ancestry cases (#3455): a changed line whose hunk replaced a base
/// region already inside a clone fragment is excused even when the change
/// altered the fragment's AST, and so its fingerprint (rustfmt bracing a
/// closure body it splits across lines). Every case covers head lines with a
/// fresh-fingerprint fragment at rows 10..=20, so only ancestry can excuse.
#[test]
fn diff_gate_hunk_ancestry() {
    struct Case {
        why: &'static str,
        spans: &'static [(usize, usize)],
        lines: &'static [usize],
        new_side: (usize, usize),
        old_side: (usize, usize),
        charged: usize,
        excused: usize,
    }
    let cases = [
        Case {
            why: "an AST-altering reformat's lines map back into a base clone fragment",
            spans: &[(5, 15)],
            lines: &[12, 13],
            new_side: (11, 11),
            old_side: (8, 4),
            charged: 0,
            excused: 2,
        },
        Case {
            why: "a pure insertion (empty old side) replaced nothing: no ancestry",
            spans: &[(1, 100)],
            lines: &[12],
            new_side: (12, 1),
            old_side: (11, 0),
            charged: 1,
            excused: 0,
        },
        Case {
            why: "ancestry needs the old side to overlap a base fragment span",
            spans: &[(50, 60)],
            lines: &[12],
            new_side: (11, 5),
            old_side: (11, 5),
            charged: 1,
            excused: 0,
        },
    ];
    for case in cases {
        let r = result(0.0, vec![group_fp("a.rs", 10, 20, 7)]);
        let ch = changed_via_hunk(case.lines, case.new_side, case.old_side);
        let g = eval(&r, &ch, 0.0, &base_spans(case.spans));
        assert_eq!(g.duplicated_changed_lines, case.charged, "{}", case.why);
        assert_eq!(
            g.preexisting_duplicated_changed_lines, case.excused,
            "{}",
            case.why
        );
        assert_eq!(g.pass, case.charged == 0, "{}", case.why);
    }
}

/// The CLI's base-scan trigger: true only when a changed line lands inside a
/// surviving fragment.
#[test]
fn changed_lines_touch_clones_detects_overlap() {
    let r = result(0.0, vec![group("a.rs", 10, 20)]);
    assert!(changed_lines_touch_clones(&r, &changed(&[("a.rs", &[11])])));
    assert!(!changed_lines_touch_clones(&r, &changed(&[("a.rs", &[5])])));
    assert!(!changed_lines_touch_clones(
        &r,
        &changed(&[("b.rs", &[11])])
    ));
}
