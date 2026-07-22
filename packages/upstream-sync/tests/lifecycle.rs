//! Hermetic lifecycle test for the branch no other check reaches: the --open
//! recording path and the merged->retired transition run ONLY after a real
//! upstream PR exists, so a bug there surfaces on first outward use,
//! orphaning an opened-but-untracked PR and inviting a duplicate on the next
//! run. The fork repo is a REAL local git repo in the megamerge layout
//! (series read for real); gh and upstream-pr are stubbed, so the whole PR
//! lifecycle runs sandboxed with no network: open + record, merged ->
//! retired, idempotent re-run, and the orphaned-intent-key refusal.

mod common;

use std::fs;

use common::{Fixture, SUBJECT, mapping_json, run_bin, status_json, stub_path, write_stub};

const GH_STUB: &str = r#"case "$1 $2" in
  "search prs") echo "[]" ;;
  "pr view") cat "$GH_PR_VIEW_RESPONSE" ;;
  *) echo "stub gh: unexpected: $*" >&2; exit 1 ;;
esac"#;

const UPSTREAM_PR_STUB: &str = r#"echo "upstream-pr: stub invoked with: $*"
echo "  https://github.com/fakeorg/fakerepo/compare/main...fakefork:fakerepo:branch?expand=1"
echo "https://github.com/fakeorg/fakerepo/pull/99999""#;

/// Run the binary and assert it exited 0, labeled by lifecycle stage; the
/// stages share this so the test stays within clippy's function-length budget.
fn run_ok(exe: &str, args: &[&str], cwd: &std::path::Path, envs: &[(&str, String)], stage: u8) {
    let run = run_bin(exe, args, cwd, envs);
    assert_eq!(
        run.status, 0,
        "stage {stage} failed:\n{}\n{}",
        run.stdout, run.stderr
    );
}

#[test]
fn pr_lifecycle_open_retire_idempotent() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    let fixture = Fixture::new(root, &[(SUBJECT, common::BODY)]);
    let stubs = root.join("stubs");
    fs::create_dir(&stubs).unwrap();
    write_stub(&stubs, "gh", GH_STUB);
    write_stub(&stubs, "upstream-pr", UPSTREAM_PR_STUB);
    let work = root.join("work");
    fs::create_dir(&work).unwrap();
    let mapping = work.join("mapping.json");
    fs::write(
        &mapping,
        mapping_json(
            "fake",
            &format!(r#"{{"{SUBJECT}":{{"upstream":"attempt","reason":"lifecycle test"}}}}"#),
        ),
    )
    .unwrap();
    let mapping_arg = mapping.display().to_string();
    let mut envs = fixture.envs();
    envs.push(("PATH", stub_path(&stubs)));
    let exe = env!("CARGO_BIN_EXE_upstream-sync");

    // stage 1: --open records the created PR.
    run_ok(
        exe,
        &["--open", "--mapping", &mapping_arg, "fake"],
        &work,
        &envs,
        1,
    );
    let entry = &status_json(&work, "fake")["patches"][SUBJECT];
    assert_eq!(
        entry["pr"]["number"], 99999,
        "stage 1: PR not recorded: {entry}"
    );
    assert_eq!(
        entry["pr"]["state"], "draft",
        "stage 1: PR not draft: {entry}"
    );
    assert_eq!(entry["retired"], false, "stage 1: retired early: {entry}");

    // stage 2: merged upstream -> retired.
    let view = work.join("pr-view.json");
    fs::write(
        &view,
        r#"{"state":"MERGED","isDraft":false,"url":"https://github.com/fakeorg/fakerepo/pull/99999","number":99999}"#,
    )
    .unwrap();
    let mut envs_merged = envs.clone();
    envs_merged.push(("GH_PR_VIEW_RESPONSE", view.display().to_string()));
    run_ok(
        exe,
        &["--mapping", &mapping_arg, "fake"],
        &work,
        &envs_merged,
        2,
    );
    let doc = status_json(&work, "fake");
    let entry = &doc["patches"][SUBJECT];
    assert_eq!(
        entry["pr"]["state"], "merged",
        "stage 2: not merged: {entry}"
    );
    assert_eq!(entry["retired"], true, "stage 2: not retired: {entry}");
    assert_eq!(
        doc["log"].as_array().unwrap().len(),
        3,
        "stage 2: expected 3 log transitions, got {}",
        doc["log"]
    );

    // stage 3: re-run is idempotent (no duplicate transitions).
    run_ok(
        exe,
        &["--mapping", &mapping_arg, "fake"],
        &work,
        &envs_merged,
        3,
    );
    let doc = status_json(&work, "fake");
    assert_eq!(
        doc["log"].as_array().unwrap().len(),
        3,
        "stage 3: log grew on a no-change re-run: {}",
        doc["log"]
    );
}

/// An intent key that names no commit subject on the bookmark is dead
/// intent: a jj rebase that retitled the commit silently orphans the
/// authorization it encodes, so the run must fail loudly, not skip.
#[test]
fn orphaned_intent_key_fails_loudly() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    let fixture = Fixture::new(root, &[(SUBJECT, common::BODY)]);
    let work = root.join("work");
    std::fs::create_dir(&work).unwrap();
    let mapping = work.join("mapping.json");
    std::fs::write(
        &mapping,
        mapping_json(
            "fake",
            r#"{"a subject no commit carries":{"upstream":"attempt","reason":"t"}}"#,
        ),
    )
    .unwrap();
    let mut envs = fixture.envs();
    envs.push(("PATH", std::env::var("PATH").unwrap()));
    let run = run_bin(
        env!("CARGO_BIN_EXE_upstream-sync"),
        &["--mapping", &mapping.display().to_string(), "fake"],
        &work,
        &envs,
    );
    assert_ne!(run.status, 0, "orphaned key must fail:\n{}", run.stdout);
    let combined = format!("{}{}", run.stdout, run.stderr);
    assert!(
        combined.contains("a subject no commit carries"),
        "error must name the orphaned key:\n{combined}"
    );
}
