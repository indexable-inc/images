//! End-to-end diff gate: build a temp git repo, commit a base, add a function
//! that duplicates an existing one, and assert the diff gate fails (the added
//! lines are all duplicated) while a permissive global budget passes.

use std::{
    path::Path,
    process::{Command, Output},
};

use serde_json::Value;
use tempfile::TempDir;

/// A Rust function large enough to clear the default `min_lines`/`min_nodes`
/// clone thresholds.
const ORIGINAL: &str = "\
fn alpha(input: i64) -> i64 {
    let mut total = 0;
    for step in 0..input {
        total += step * 2;
        total -= 1;
    }
    total + 42
}
";

/// A byte-for-byte-structural duplicate under a different name: a Type-2 clone
/// of `ORIGINAL` (identical modulo the identifiers), so its lines land in a
/// clone group.
const DUPLICATE: &str = "\
fn beta(value: i64) -> i64 {
    let mut sum = 0;
    for count in 0..value {
        sum += count * 2;
        sum -= 1;
    }
    sum + 42
}
";

fn git(dir: &Path, args: &[&str]) -> Output {
    let output = Command::new("git")
        .current_dir(dir)
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_SYSTEM", "/dev/null")
        .env("GIT_AUTHOR_NAME", "t")
        .env("GIT_AUTHOR_EMAIL", "t@t")
        .env("GIT_COMMITTER_NAME", "t")
        .env("GIT_COMMITTER_EMAIL", "t@t")
        .args(args)
        .output()
        .expect("git should run");
    assert!(
        output.status.success(),
        "git {args:?} failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    output
}

/// The parsed result of a `clone` invocation.
struct CloneRun {
    json: Value,
    success: bool,
}

/// Run the `clone` binary in `dir` with the given args, returning parsed JSON
/// stdout and the exit success flag.
fn run_clone(dir: &Path, args: &[&str]) -> CloneRun {
    let output = Command::new(env!("CARGO_BIN_EXE_clone"))
        .current_dir(dir)
        .args(args)
        .output()
        .expect("clone binary should run");
    let stdout = String::from_utf8(output.stdout).expect("clone stdout is UTF-8");
    let json: Value = serde_json::from_str(&stdout)
        .unwrap_or_else(|e| panic!("clone stdout is not JSON ({e}): {stdout}"));
    CloneRun {
        json,
        success: output.status.success(),
    }
}

#[test]
fn diff_gate_fails_on_duplicated_change_while_global_passes() {
    let repo = TempDir::new().expect("tempdir");
    let dir = repo.path();

    // A clone.toml that lowers the thresholds enough for the small fixtures to
    // register as clones, and does not ignore the source files.
    std::fs::write(
        dir.join("clone.toml"),
        "min_lines = 3\nmin_nodes = 5\n[budget]\nglobal_pct = 100.0\ndiff_pct = 0.0\n",
    )
    .unwrap();

    git(dir, &["init", "-q"]);
    // Base commit: one function, no duplication yet.
    std::fs::write(dir.join("original.rs"), ORIGINAL).unwrap();
    git(dir, &["add", "-A"]);
    git(dir, &["commit", "-qm", "base"]);

    // The change under test: add a duplicate function in a new file. Its lines
    // are all "changed" (added) and all part of a clone group.
    std::fs::write(dir.join("duplicate.rs"), DUPLICATE).unwrap();

    // Global budget is permissive (100%), diff budget is 0%. Diff base is HEAD:
    // merge-base(HEAD, HEAD) == HEAD, so the diff is HEAD-tree vs the worktree,
    // i.e. the uncommitted duplicate.
    let run = run_clone(dir, &["--diff", "HEAD", ".", "--pretty"]);
    let json = &run.json;

    let global = &json["gate"]["global"];
    assert_eq!(
        global["pass"], Value::Bool(true),
        "global gate should pass under a 100% budget: {json:#}"
    );

    let diff = &json["gate"]["diff"];
    assert_eq!(
        diff["pass"], Value::Bool(false),
        "diff gate should fail: the added function duplicates the base: {json:#}"
    );
    assert!(
        diff["changed_lines"].as_u64().unwrap() > 0,
        "the added file must contribute changed lines: {json:#}"
    );
    assert!(
        diff["duplicated_changed_lines"].as_u64().unwrap() > 0,
        "the added duplicate must cover some changed lines: {json:#}"
    );

    // Exit code follows the worst gate: a failing diff gate means failure.
    assert!(
        !run.success,
        "clone should exit nonzero when the diff gate fails"
    );
}

#[test]
fn diff_gate_passes_when_change_is_not_duplicated() {
    let repo = TempDir::new().expect("tempdir");
    let dir = repo.path();

    std::fs::write(
        dir.join("clone.toml"),
        "min_lines = 3\nmin_nodes = 5\n[budget]\nglobal_pct = 100.0\ndiff_pct = 0.0\n",
    )
    .unwrap();

    git(dir, &["init", "-q"]);
    std::fs::write(dir.join("original.rs"), ORIGINAL).unwrap();
    git(dir, &["add", "-A"]);
    git(dir, &["commit", "-qm", "base"]);

    // Add a unique, non-duplicated function: its changed lines are not covered
    // by any clone, so the diff gate passes even at a 0% budget.
    std::fs::write(
        dir.join("unique.rs"),
        "fn gamma() -> &'static str {\n    \"a wholly unique body\"\n}\n",
    )
    .unwrap();

    let run = run_clone(dir, &["--diff", "HEAD", "."]);
    let json = &run.json;
    assert_eq!(
        json["gate"]["diff"]["pass"],
        Value::Bool(true),
        "diff gate should pass when the change is not duplicated: {json:#}"
    );
    assert!(run.success, "clone should exit zero when all gates pass");
}

#[test]
fn diff_gate_fails_loudly_on_unknown_base() {
    let repo = TempDir::new().expect("tempdir");
    let dir = repo.path();

    std::fs::write(dir.join("clone.toml"), "min_lines = 3\nmin_nodes = 5\n").unwrap();
    git(dir, &["init", "-q"]);
    std::fs::write(dir.join("original.rs"), ORIGINAL).unwrap();
    git(dir, &["add", "-A"]);
    git(dir, &["commit", "-qm", "base"]);

    // A base rev that does not exist must fail the run, never silently skip.
    let output = Command::new(env!("CARGO_BIN_EXE_clone"))
        .current_dir(dir)
        .args(["--diff", "definitely-not-a-real-ref", "."])
        .output()
        .expect("clone binary should run");
    assert!(
        !output.status.success(),
        "clone must exit nonzero when the diff base is unknown"
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("merge base") || stderr.contains("definitely-not-a-real-ref"),
        "error should name the missing base: {stderr}"
    );
}
