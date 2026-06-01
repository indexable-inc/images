//! End-to-end proof of the bench loop: run -> record to history -> a second run
//! compares against the previous one -> report.
//!
//! This drives the library surface a consumer uses (it does not shell out to the
//! CLI), so it stays a fast, hermetic `#[test]`. It uses the [`LocalDirStore`]
//! in a tempdir rather than the git-branch store so the test needs no repo and
//! leaves nothing behind.

use indexbench::compare::{CompareConfig, Verdict, compare};
use indexbench::report::human_table;
use indexbench::run::{GitContext, execute};
use indexbench::store::{HistoryStore, LocalDirStore};
use indexbench::suite::{BenchSuite, MacroBench, MicroBench};

/// Run a self-demo suite (one micro fn, one macro `true`), record it, run a
/// second time, and assert the second run finds the first as its baseline and
/// produces a comparison the reporter can render.
#[test]
fn run_record_compare_report_round_trip() {
    let dir = tempfile::tempdir().expect("tempdir");
    let store = LocalDirStore::new(dir.path()).expect("store");

    let git_first = GitContext {
        commit: "commit-one".to_owned(),
        dirty: false,
    };

    // First run: establishes the baseline. There is nothing prior, so the
    // previous-run lookup (read before recording) returns None.
    let mut suite = demo_suite();
    let first_runs = execute(&mut suite, &git_first).expect("first execute");
    assert_eq!(first_runs.len(), 2, "one run per bench");
    for run in &first_runs {
        let baseline = store.previous_run(&run.suite, &run.bench, &run.machine_id).expect("baseline read");
        assert!(baseline.is_none(), "the first run has no baseline");
        store.append(run).expect("record first run");
    }

    // Second run at a later commit: each bench should now find the first run as
    // its baseline and yield a comparison.
    let git_second = GitContext {
        commit: "commit-two".to_owned(),
        dirty: false,
    };
    let mut suite = demo_suite();
    let second_runs = execute(&mut suite, &git_second).expect("second execute");

    let mut compared = 0;
    for run in &second_runs {
        // Read the baseline before recording (the CLI's order), so the run is
        // never its own baseline.
        let baseline = store
            .previous_run(&run.suite, &run.bench, &run.machine_id)
            .expect("baseline read")
            .expect("second run has a baseline");
        store.append(run).expect("record second run");
        assert_eq!(baseline.git_commit, "commit-one", "baseline is the previous run on this machine");

        let comparison = compare(&baseline, run, CompareConfig::default());
        // Every metric must classify into one of the known verdicts; none should
        // be left unclassified.
        assert!(!comparison.metrics.is_empty(), "comparison carries metrics for {}", run.bench);
        for metric in &comparison.metrics {
            assert!(
                matches!(
                    metric.verdict,
                    Verdict::Improvement | Verdict::Regression | Verdict::Unchanged | Verdict::NoBaseline
                ),
                "metric {} classified",
                metric.name
            );
        }

        let table = human_table(&comparison);
        assert!(table.contains(&run.bench), "report names the bench: {table}");
        compared += 1;
    }
    assert_eq!(compared, 2, "both benches compared against a baseline");

    // History now holds two runs per bench.
    let fib_history = store.runs_for("self-demo", "fib").expect("fib history");
    assert_eq!(fib_history.len(), 2);
    let true_history = store.runs_for("self-demo", "true").expect("true history");
    assert_eq!(true_history.len(), 2);
}

/// A deterministic regression on a recorded baseline fails the gate.
///
/// We record a baseline with a low allocation count, then a candidate with a
/// higher one, and assert the comparison reports a regression — the exact
/// behavior the CLI maps to a non-zero exit.
#[test]
fn deterministic_regression_against_history_is_gated() {
    use indexbench::schema::{Metric, Run};

    let dir = tempfile::tempdir().expect("tempdir");
    let store = LocalDirStore::new(dir.path()).expect("store");

    let make = |timestamp: i64, allocations: f64| Run {
        suite: "alloc-suite".to_owned(),
        bench: "build-index".to_owned(),
        metrics: vec![Metric::deterministic("allocations", allocations, "count", true)],
        machine_id: "machine-a".to_owned(),
        git_commit: format!("c{timestamp}"),
        git_dirty: false,
        timestamp_unix: timestamp,
    };

    store.append(&make(100, 1000.0)).expect("baseline");
    let candidate = make(200, 1100.0);
    store.append(&candidate).expect("candidate");

    let baseline = previous_excluding_self(&store, &candidate).expect("baseline present");
    let comparison = compare(&baseline, &candidate, CompareConfig::default());
    assert!(comparison.has_regression(), "a higher allocation count must gate as a regression");
}

/// The suite used by the round-trip test: one micro Rust fn and one trivial
/// macro command. Built fresh each call because [`MicroBench`] owns a `FnMut`
/// closure that `execute` consumes.
fn demo_suite() -> BenchSuite<'static> {
    BenchSuite::new("self-demo")
        .micro(MicroBench::new("fib", || {
            std::hint::black_box(fib(std::hint::black_box(15)));
        }))
        .macro_bench(MacroBench::new("true", "true", Vec::<String>::new()).with_runs(3))
}

/// The previous run for this bench on this machine, excluding the run itself.
/// Mirrors the CLI's baseline selection (drop the most-recent identical entry,
/// take the last of what remains) so the test exercises the real comparison
/// path even when two runs share a whole-second timestamp.
fn previous_excluding_self(store: &LocalDirStore, current: &indexbench::Run) -> Option<indexbench::Run> {
    let mut runs = store.runs_for(&current.suite, &current.bench).expect("history read");
    runs.retain(|candidate| candidate.machine_id == current.machine_id);
    if let Some(position) = runs.iter().rposition(|candidate| candidate == current) {
        runs.remove(position);
    }
    runs.pop()
}

/// Small CPU-bound function so the micro bench has real work to time.
fn fib(n: u64) -> u64 {
    match n {
        0 => 0,
        1 => 1,
        _ => fib(n - 1) + fib(n - 2),
    }
}
