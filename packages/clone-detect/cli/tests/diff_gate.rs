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

/// A temp git repo whose clone.toml lowers the thresholds enough for the small
/// fixtures to register as clones and grants the whole budget to the global
/// gate (100%) while zeroing the diff gate. `base` files form the base commit;
/// `head` files are then written over the working tree as the uncommitted
/// change under test.
fn repo(base: &[(&str, &str)], head: &[(&str, &str)]) -> TempDir {
    let tempdir = TempDir::new().expect("tempdir");
    let dir = tempdir.path();
    std::fs::write(
        dir.join("clone.toml"),
        "min_lines = 3\nmin_nodes = 5\n[budget]\nglobal_pct = 100.0\ndiff_pct = 0.0\n",
    )
    .unwrap();
    git(dir, &["init", "-q"]);
    for (file, source) in base {
        std::fs::write(dir.join(file), source).unwrap();
    }
    git(dir, &["add", "-A"]);
    git(dir, &["commit", "-qm", "base"]);
    for (file, source) in head {
        std::fs::write(dir.join(file), source).unwrap();
    }
    tempdir
}

/// Assert a reflow-only change registers changed lines yet zero NEW
/// duplication, so the run passes the 0% diff budget.
fn assert_reformat_excused(dir: &Path) {
    let run = run_clone(dir, &["--diff", "HEAD", ".", "--pretty"]);
    let json = &run.json;
    let diff = &json["gate"]["diff"];
    assert!(
        diff["changed_lines"].as_u64().unwrap() > 0,
        "the reflow must register as changed lines: {json:#}"
    );
    assert_eq!(
        diff["duplicated_changed_lines"].as_u64().unwrap(),
        0,
        "a reformat of pre-existing duplication is not NEW duplication: {json:#}"
    );
    assert!(run.success, "clone should exit zero: {json:#}");
}

/// Assert the working-tree change is charged as new duplication: the diff
/// gate fails and the run exits nonzero.
fn assert_new_duplication_charged(dir: &Path, why: &str) {
    let run = run_clone(dir, &["--diff", "HEAD", "."]);
    let json = &run.json;
    assert_eq!(
        json["gate"]["diff"]["pass"],
        Value::Bool(false),
        "{why}: {json:#}"
    );
    assert!(!run.success, "clone should exit nonzero: {json:#}");
}

#[test]
fn diff_gate_fails_on_duplicated_change_while_global_passes() {
    // The change under test: add a duplicate function in a new file. Its lines
    // are all "changed" (added) and all part of a clone group.
    let repo = repo(&[("original.rs", ORIGINAL)], &[("duplicate.rs", DUPLICATE)]);
    let dir = repo.path();

    // Diff base is HEAD: merge-base(HEAD, HEAD) == HEAD, so the diff is
    // HEAD-tree vs the worktree, i.e. the uncommitted duplicate.
    let run = run_clone(dir, &["--diff", "HEAD", ".", "--pretty"]);
    let json = &run.json;

    let global = &json["gate"]["global"];
    assert_eq!(
        global["pass"],
        Value::Bool(true),
        "global gate should pass under a 100% budget: {json:#}"
    );

    let diff = &json["gate"]["diff"];
    assert_eq!(
        diff["pass"],
        Value::Bool(false),
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
    // Add a unique, non-duplicated function: its changed lines are not covered
    // by any clone, so the diff gate passes even at a 0% budget.
    let repo = repo(
        &[("original.rs", ORIGINAL)],
        &[(
            "unique.rs",
            "fn gamma() -> &'static str {\n    \"a wholly unique body\"\n}\n",
        )],
    );
    let dir = repo.path();

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
    let repo = repo(&[("original.rs", ORIGINAL)], &[]);
    let dir = repo.path();

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

/// `ORIGINAL` with its loop body packed onto one line: the same token stream
/// (identical AST), reflowed. Committing this and then restoring `ORIGINAL`
/// in the working tree is a pure reformat of a pre-existing clone fragment.
const ORIGINAL_PACKED: &str = "\
fn alpha(input: i64) -> i64 {
    let mut total = 0;
    for step in 0..input {
        total += step * 2; total -= 1;
    }
    total + 42
}
";

/// A third structural twin of `ORIGINAL`/`DUPLICATE` under fresh names, used
/// to prove that a NEW copy of an already-duplicated shape still fails.
const TRIPLICATE: &str = "\
fn gamma(amount: i64) -> i64 {
    let mut acc = 0;
    for tick in 0..amount {
        acc += tick * 2;
        acc -= 1;
    }
    acc + 42
}
";

/// Regression test for #3455: a reformat of a clone that already existed at
/// the diff base must not count as new duplication. The base commit holds a
/// clone pair (`alpha` packed, `beta`); the working tree only reflows `alpha`
/// (same AST, new line breaks), exactly what a tree-wide `cargo fmt` lane
/// does. Before base-awareness this read as duplicated changed lines and
/// failed the 0% diff budget.
#[test]
fn diff_gate_passes_on_reformat_of_preexisting_clone() {
    // Base commit: the clone pair already exists. The change under test:
    // reflow one fragment of it.
    let repo = repo(
        &[
            ("original.rs", ORIGINAL_PACKED),
            ("duplicate.rs", DUPLICATE),
        ],
        &[("original.rs", ORIGINAL)],
    );
    assert_reformat_excused(repo.path());
}

/// Control for base-awareness: a NEW copy of a shape that was already cloned
/// at base is new duplication and must still fail. Guards the identity choice
/// (file + fingerprint): the fingerprint alone exists at base, but not in the
/// added file.
#[test]
fn diff_gate_fails_on_new_copy_of_preexisting_clone() {
    // The change under test: a third copy in a new file.
    let repo = repo(
        &[("original.rs", ORIGINAL), ("duplicate.rs", DUPLICATE)],
        &[("triplicate.rs", TRIPLICATE)],
    );
    assert_new_duplication_charged(
        repo.path(),
        "a new copy of an already-duplicated shape is new duplication",
    );
}

/// Same-file control: appending the third copy to a file that already held
/// one twin must also fail. The fingerprint exists at base in this very file,
/// so this guards the multiplicity rule (base had one copy here, head has
/// two) and the no-ancestry rule for insertions.
#[test]
fn diff_gate_fails_on_same_file_copy_of_preexisting_clone() {
    // The change under test: a third copy appended to an existing file.
    let appended = format!("{DUPLICATE}\n{TRIPLICATE}");
    let repo = repo(
        &[("original.rs", ORIGINAL), ("duplicate.rs", DUPLICATE)],
        &[("duplicate.rs", &appended)],
    );
    assert_new_duplication_charged(
        repo.path(),
        "a same-file copy of an already-duplicated shape is new duplication",
    );
}

/// `alpha`/`beta` clone pair whose closure body sits on one line: the shape
/// rustfmt rewrites by bracing the body once it splits across lines, which
/// legitimately CHANGES the AST (and so the fragments' fingerprints).
const CLOSURE_PACKED: [(&str, &str); 2] = [
    (
        "original.rs",
        "\
fn alpha(input: i64) -> i64 {
    let mut total = 0;
    let bump = |x: i64| input * 2 + x * 3 + 1;
    for step in 0..input {
        total += bump(step);
    }
    total
}
",
    ),
    (
        "duplicate.rs",
        "\
fn beta(value: i64) -> i64 {
    let mut sum = 0;
    let grow = |x: i64| value * 2 + x * 3 + 1;
    for count in 0..value {
        sum += grow(count);
    }
    sum
}
",
    ),
];

/// [`CLOSURE_PACKED`] after a rustfmt-style reflow: the closure body gains a
/// block, so the AST (and every fingerprint containing it) differs from base.
const CLOSURE_REFLOWED: [(&str, &str); 2] = [
    (
        "original.rs",
        "\
fn alpha(input: i64) -> i64 {
    let mut total = 0;
    let bump = |x: i64| {
        input * 2 + x * 3 + 1
    };
    for step in 0..input {
        total += bump(step);
    }
    total
}
",
    ),
    (
        "duplicate.rs",
        "\
fn beta(value: i64) -> i64 {
    let mut sum = 0;
    let grow = |x: i64| {
        value * 2 + x * 3 + 1
    };
    for count in 0..value {
        sum += grow(count);
    }
    sum
}
",
    ),
];

/// #3455, the fingerprint-breaking reformat: rustfmt braces a closure body it
/// splits across lines, so the reflowed fragments carry NEW fingerprints.
/// Hunk ancestry must excuse them: every changed line replaced a base region
/// that was already inside a clone fragment.
#[test]
fn diff_gate_passes_on_reformat_that_alters_the_ast() {
    let repo = repo(&CLOSURE_PACKED, &CLOSURE_REFLOWED);
    assert_reformat_excused(repo.path());
}

/// One-line clone pair, below `min_lines = 3` at the base so the base scan at
/// the configured threshold would never report it.
const TINY_PACKED: [(&str, &str); 2] = [
    (
        "original.rs",
        "fn alpha(input: i64) -> i64 { (input * 2 + 7) * (input - 3) + input * input }\n",
    ),
    (
        "duplicate.rs",
        "fn beta(value: i64) -> i64 { (value * 2 + 7) * (value - 3) + value * value }\n",
    ),
];

/// [`TINY_PACKED`] reflowed across three lines: same AST, now over the
/// reporting threshold.
const TINY_REFLOWED: [(&str, &str); 2] = [
    (
        "original.rs",
        "fn alpha(input: i64) -> i64 {\n    (input * 2 + 7) * (input - 3) + input * input\n}\n",
    ),
    (
        "duplicate.rs",
        "fn beta(value: i64) -> i64 {\n    (value * 2 + 7) * (value - 3) + value * value\n}\n",
    ),
];

/// #3455, the threshold-crossing reflow: a clone pair packed under `min_lines`
/// at the base becomes reportable purely by gaining line breaks. The base scan
/// relaxes `min_lines` to 1 so the pair still registers as pre-existing.
#[test]
fn diff_gate_passes_when_a_reflow_crosses_the_min_lines_threshold() {
    let repo = repo(&TINY_PACKED, &TINY_REFLOWED);
    assert_reformat_excused(repo.path());
}
