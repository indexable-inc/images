//! Transparency tests: the wrapper must be byte- and status-identical to the
//! bare tool, and every invocation must land in the local store.

use std::path::PathBuf;
use std::process::{Command, Output};

/// The `counts` row for the demo package: how many wrapped invocations ran
/// and how many exited nonzero.
#[derive(Debug, PartialEq, Eq)]
struct Counts {
    invocations: i64,
    nonzero_exits: i64,
}

struct Harness {
    dir: tempfile::TempDir,
}

impl Harness {
    fn new(mode: &str) -> Self {
        let dir = tempfile::tempdir().expect("tempdir");
        let spec = serde_json::json!({
            "target": "sh",
            "pkg": "demo-pkg",
            "version": "1.2.3",
            "mode": mode,
            "errors": true,
        });
        std::fs::write(dir.path().join("spec.json"), spec.to_string()).expect("write spec");
        // Pre-decide consent so no first-run notice lands on stderr and no
        // upload kick can spawn.
        std::fs::write(dir.path().join("usage.toml"), "enabled = false\n").expect("write config");
        Self { dir }
    }

    fn state_dir(&self) -> PathBuf {
        self.dir.path().join("state")
    }

    fn run(&self, args: &[&str]) -> Output {
        self.command().args(args).output().expect("run ix-wrap")
    }

    fn command(&self) -> Command {
        let mut command = Command::new(env!("CARGO_BIN_EXE_ix-wrap"));
        command
            .env("IX_USAGE_SPEC", self.dir.path().join("spec.json"))
            .env("IX_USAGE_STATE_DIR", self.state_dir())
            .env("IX_USAGE_CONFIG", self.dir.path().join("usage.toml"));
        command
    }

    fn counts(&self) -> Counts {
        ix_usage_core::store::compact(&self.state_dir()).expect("compact");
        let conn = ix_usage_core::store::open(&self.state_dir().join("usage.db")).expect("open db");
        conn.query_row(
            "SELECT invocations, nonzero_exits FROM counts
             WHERE pkg = 'demo-pkg' AND version = '1.2.3'",
            [],
            |row| {
                Ok(Counts {
                    invocations: row.get(0)?,
                    nonzero_exits: row.get(1)?,
                })
            },
        )
        .expect("counts row")
    }

    #[track_caller]
    fn assert_counts(&self, invocations: i64, nonzero_exits: i64) {
        assert_eq!(
            self.counts(),
            Counts {
                invocations,
                nonzero_exits,
            }
        );
    }

    /// Run the wrapped shell and assert the pass-through exit code and
    /// stdout in one place, returning the output for extra assertions.
    fn run_expecting(&self, args: &[&str], code: i32, stdout: &[u8]) -> Output {
        let output = self.run(args);
        assert_eq!(output.status.code(), Some(code));
        assert_eq!(output.stdout, stdout);
        output
    }

    fn error_rows(&self) -> i64 {
        let conn = ix_usage_core::store::open(&self.state_dir().join("usage.db")).expect("open db");
        conn.query_row("SELECT COUNT(*) FROM errors", [], |row| row.get(0))
            .expect("count errors")
    }
}

#[test]
fn passes_through_exit_code_stdout_and_records_failure() {
    let harness = Harness::new("observe");
    let output = harness.run_expecting(&["-c", "printf hello; exit 7"], 7, b"hello");
    assert_eq!(output.stderr, b"", "wrapper must not write to stderr");

    harness.assert_counts(1, 1);
    assert_eq!(harness.error_rows(), 1);
}

#[test]
fn success_records_count_without_error_row() {
    let harness = Harness::new("observe");
    harness.run_expecting(&["-c", "printf ok"], 0, b"ok");

    harness.assert_counts(1, 0);
    assert_eq!(harness.error_rows(), 0);
}

#[test]
fn signal_death_maps_to_128_plus_signal() {
    let harness = Harness::new("observe");
    let output = harness.run(&["-c", "kill -TERM $$"]);
    assert_eq!(output.status.code(), Some(143), "SIGTERM death is 128+15");

    harness.assert_counts(1, 1);
}

#[test]
fn count_only_execs_and_records_invocation_only() {
    let harness = Harness::new("count-only");
    harness.run_expecting(&["-c", "printf fast; exit 5"], 5, b"fast");

    // Exit is unknown by design in count-only mode.
    harness.assert_counts(1, 0);
    assert_eq!(harness.error_rows(), 0);
}

#[test]
fn repeated_runs_accumulate() {
    let harness = Harness::new("observe");
    for _ in 0..5 {
        assert_eq!(harness.run(&["-c", "exit 0"]).status.code(), Some(0));
    }
    assert_eq!(harness.run(&["-c", "exit 1"]).status.code(), Some(1));
    harness.assert_counts(6, 1);
}

#[test]
fn missing_spec_fails_loud_with_127() {
    let harness = Harness::new("observe");
    let mut command = harness.command();
    command.env_remove("IX_USAGE_SPEC");
    let output = command.args(["-c", "exit 0"]).output().expect("run");
    assert_eq!(output.status.code(), Some(127));
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("IX_USAGE_SPEC"),
        "actionable error, got: {stderr}"
    );
}

#[test]
fn argv_of_failures_stays_local() {
    let harness = Harness::new("observe");
    let output = harness.run(&["-c", "exit 9"]);
    assert_eq!(output.status.code(), Some(9));
    let _ = harness.counts();
    let conn = ix_usage_core::store::open(&harness.state_dir().join("usage.db")).expect("open");
    let argv: String = conn
        .query_row("SELECT argv FROM errors", [], |row| row.get(0))
        .expect("argv column");
    assert!(argv.contains("exit 9"), "argv captured locally: {argv}");

    let report = ix_usage_core::payload::build_report(&conn, "1970-01-01", false).expect("report");
    let wire = serde_json::to_string(&report).expect("serialize");
    assert!(
        !wire.contains("exit 9"),
        "argv must never enter the upload payload"
    );
}
