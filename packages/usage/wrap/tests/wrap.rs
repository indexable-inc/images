//! Transparency tests: the wrapper must be byte- and status-identical to the
//! bare tool, and every invocation must land in the local store.

use std::path::PathBuf;
use std::process::{Command, Output};

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

    fn counts(&self) -> (i64, i64) {
        ix_usage_core::store::compact(&self.state_dir()).expect("compact");
        let conn = ix_usage_core::store::open(&self.state_dir().join("usage.db")).expect("open db");
        conn.query_row(
            "SELECT invocations, nonzero_exits FROM counts
             WHERE pkg = 'demo-pkg' AND version = '1.2.3'",
            [],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .expect("counts row")
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
    let output = harness.run(&["-c", "printf hello; exit 7"]);
    assert_eq!(output.status.code(), Some(7));
    assert_eq!(output.stdout, b"hello");
    assert_eq!(output.stderr, b"", "wrapper must not write to stderr");

    assert_eq!(harness.counts(), (1, 1));
    assert_eq!(harness.error_rows(), 1);
}

#[test]
fn success_records_count_without_error_row() {
    let harness = Harness::new("observe");
    let output = harness.run(&["-c", "printf ok"]);
    assert_eq!(output.status.code(), Some(0));
    assert_eq!(output.stdout, b"ok");

    assert_eq!(harness.counts(), (1, 0));
    assert_eq!(harness.error_rows(), 0);
}

#[test]
fn signal_death_maps_to_128_plus_signal() {
    let harness = Harness::new("observe");
    let output = harness.run(&["-c", "kill -TERM $$"]);
    assert_eq!(output.status.code(), Some(143), "SIGTERM death is 128+15");

    assert_eq!(harness.counts(), (1, 1));
}

#[test]
fn count_only_execs_and_records_invocation_only() {
    let harness = Harness::new("count-only");
    let output = harness.run(&["-c", "printf fast; exit 5"]);
    assert_eq!(output.status.code(), Some(5));
    assert_eq!(output.stdout, b"fast");

    // Exit is unknown by design in count-only mode.
    assert_eq!(harness.counts(), (1, 0));
    assert_eq!(harness.error_rows(), 0);
}

#[test]
fn repeated_runs_accumulate() {
    let harness = Harness::new("observe");
    for _ in 0..5 {
        assert_eq!(harness.run(&["-c", "exit 0"]).status.code(), Some(0));
    }
    assert_eq!(harness.run(&["-c", "exit 1"]).status.code(), Some(1));
    assert_eq!(harness.counts(), (6, 1));
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
