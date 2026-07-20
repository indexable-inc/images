//! Agent-facing JSON contract tests for the `ix-usage` CLI.

use std::path::Path;
use std::process::{Command, Output};

fn run(dir: &Path, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_ix-usage"))
        .args(args)
        .env("IX_USAGE_STATE_DIR", dir.join("state"))
        .env("IX_USAGE_CONFIG", dir.join("usage.toml"))
        .env_remove("IX_USAGE")
        .env_remove("DO_NOT_TRACK")
        .output()
        .expect("run ix-usage")
}

fn json(output: &Output) -> serde_json::Value {
    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    serde_json::from_slice(&output.stdout).expect("stdout is JSON")
}

fn seed_failure(dir: &Path) {
    // Recent timestamp so the record falls inside the report window.
    let ts_ms = ix_usage_core::spool::now_ms().expect("clock");
    let record = ix_usage_core::spool::Record {
        ts_ms,
        pkg: "demo".to_owned(),
        version: "1.0".to_owned(),
        exit: Some(2),
        duration_ms: Some(4),
        argv: Some(vec!["demo".to_owned(), "--boom".to_owned()]),
        cwd: None,
    };
    ix_usage_core::spool::append_at(&dir.join("state/usage.spool"), &record).expect("seed");
}

#[test]
fn status_defaults_to_upload_on() {
    let dir = tempfile::tempdir().expect("tempdir");
    let status = json(&run(dir.path(), &["status", "--json"]));
    assert_eq!(status["upload_enabled"], true);
    assert_eq!(status["source"], "default");
}

#[test]
fn off_flips_consent_via_config() {
    let dir = tempfile::tempdir().expect("tempdir");
    assert!(run(dir.path(), &["off"]).status.success());
    let status = json(&run(dir.path(), &["status", "--json"]));
    assert_eq!(status["upload_enabled"], false);
    assert_eq!(status["source"], "config");
}

#[test]
fn errors_json_carries_the_whole_record() {
    let dir = tempfile::tempdir().expect("tempdir");
    seed_failure(dir.path());
    let errors = json(&run(dir.path(), &["errors", "--json"]));
    let rows = errors.as_array().expect("array");
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0]["pkg"], "demo");
    assert_eq!(rows[0]["exit_code"], 2);
    assert_eq!(rows[0]["argv"][1], "--boom");
}

#[test]
fn show_prints_counts_only_payload() {
    let dir = tempfile::tempdir().expect("tempdir");
    seed_failure(dir.path());
    let output = run(dir.path(), &["show"]);
    let report = json(&output);
    assert_eq!(report["v"], 1);
    assert_eq!(report["counts"].as_array().map(Vec::len), Some(1));
    let wire = String::from_utf8_lossy(&output.stdout);
    assert!(!wire.contains("--boom"), "payload must never carry argv");
}

#[test]
fn upload_dry_run_needs_no_network() {
    let dir = tempfile::tempdir().expect("tempdir");
    seed_failure(dir.path());
    let report = json(&run(dir.path(), &["upload", "--dry-run"]));
    assert_eq!(report["counts"][0]["failures"], 1);
}
