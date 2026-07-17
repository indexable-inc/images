//! Hermetic lifecycle test for the branch no other check reaches: the --open
//! recording path, CI-verdict tracking, and the merged->retired transition
//! run ONLY after a real upstream PR exists, so a bug there surfaces on
//! first outward use, orphaning an opened-but-untracked PR and inviting a
//! duplicate on the next run. gh, upstream-pr, and nix are stubbed, so the
//! whole PR lifecycle runs sandboxed with no network: open + record, red
//! upstream CI recorded + `--fail-on-red-ci` failing the run, merged ->
//! retired, idempotent re-run, and the closure-gate preflight (red aborts,
//! green proceeds).

mod common;

use std::fs;

use common::{PATCH, run_bin, status_json, stub_path, write_series, write_stub};

const GH_STUB: &str = r#"case "$1 $2" in
  "search prs") echo "[]" ;;
  "pr view") cat "$GH_PR_VIEW_RESPONSE" ;;
  *) echo "stub gh: unexpected: $*" >&2; exit 1 ;;
esac"#;

const UPSTREAM_PR_STUB: &str = r#"echo "upstream-pr: stub invoked with: $*"
echo "  https://github.com/fakeorg/fakerepo/compare/main...indexable-inc:fakerepo:branch?expand=1"
echo "https://github.com/fakeorg/fakerepo/pull/99999""#;

// `config show system` names the gate attr's system; `build` exits per
// NIX_GATE_EXIT so the stages below drive a red and a green gate through the
// REAL preflight branch.
const NIX_STUB: &str = r#"case "$1" in
  config) echo "x86_64-stub" ;;
  build) exit "${NIX_GATE_EXIT:-0}" ;;
  *) echo "stub nix: unexpected: $*" >&2; exit 1 ;;
esac"#;

// An open, non-draft PR whose upstream CI matrix went red: one real check
// failure plus a green commit-status context, the mixed rollup shape gh
// returns. The refresh must collapse this to ci = failing with the check
// name kept.
const RED_CI_VIEW: &str = r#"{"state":"OPEN","isDraft":false,"url":"https://github.com/fakeorg/fakerepo/pull/99999","number":99999,
 "statusCheckRollup":[
   {"__typename":"CheckRun","name":"cargo fmt","status":"COMPLETED","conclusion":"FAILURE"},
   {"__typename":"StatusContext","context":"ci/other","state":"SUCCESS"}]}"#;

fn mapping_json(name: &str, patch_dir: &str, closure_gates: bool) -> String {
    format!(
        r#"[{{"name":"{name}","input":"{name}-src","url":"https://github.com/fakeorg/fakerepo.git",
  "patchDir":"{patch_dir}","autoUpdate":false,"closureGates":{closure_gates},
  "upstreamPolicy":{{"prsWelcome":true,"aiPrsAllowed":"unknown","citation":"https://example.com","notes":"t"}},
  "patches":{{"{PATCH}":{{"upstream":"attempt","reason":"lifecycle test"}}}}}}]"#
    )
}

/// Run the binary and assert its exit status: 0 when `want_ok`, nonzero
/// otherwise, labeled by lifecycle stage. The stages share this so the test
/// stays within clippy's function-length budget.
fn run_expect(
    exe: &str,
    args: &[&str],
    cwd: &std::path::Path,
    envs: &[(&str, String)],
    stage: u8,
    want_ok: bool,
) {
    let run = run_bin(exe, args, cwd, envs);
    assert_eq!(
        run.status == 0,
        want_ok,
        "stage {stage}: expected {} exit, got {}:\n{}\n{}",
        if want_ok { "a zero" } else { "a nonzero" },
        run.status,
        run.stdout,
        run.stderr
    );
}

#[test]
fn pr_lifecycle_open_red_ci_retire_idempotent_and_gates() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    let stubs = root.join("stubs");
    fs::create_dir(&stubs).unwrap();
    write_stub(&stubs, "gh", GH_STUB);
    write_stub(&stubs, "upstream-pr", UPSTREAM_PR_STUB);
    write_stub(&stubs, "nix", NIX_STUB);
    let work = root.join("work");
    write_series(&work, "repo/patches");
    let mapping = work.join("mapping.json");
    fs::write(&mapping, mapping_json("fake", "repo/patches", false)).unwrap();
    let mapping_arg = mapping.display().to_string();
    let path = stub_path(&stubs);
    let exe = env!("CARGO_BIN_EXE_upstream-sync");
    let envs = [("PATH", path)];

    // stage 1: --open records the created PR, ready for review with CI still
    // pending (upstream-pr opens ready; CI has not reported yet).
    run_expect(
        exe,
        &["--open", "--mapping", &mapping_arg, "fake"],
        &work,
        &envs,
        1,
        true,
    );
    let entry = &status_json(&work, "repo/patches")["patches"][PATCH];
    assert_eq!(
        entry["pr"]["number"], 99999,
        "stage 1: PR not recorded: {entry}"
    );
    assert_eq!(
        entry["pr"]["state"], "open",
        "stage 1: PR not open/ready: {entry}"
    );
    assert_eq!(
        entry["pr"]["ci"], "pending",
        "stage 1: CI not pending: {entry}"
    );
    assert_eq!(entry["retired"], false, "stage 1: retired early: {entry}");

    // stage 2: red upstream CI is recorded (verdict + failing check names)
    // and --fail-on-red-ci fails the run, without duplicate log growth.
    let view = work.join("pr-view.json");
    fs::write(&view, RED_CI_VIEW).unwrap();
    let envs_red = [
        ("PATH", envs[0].1.clone()),
        ("GH_PR_VIEW_RESPONSE", view.display().to_string()),
    ];
    run_expect(
        exe,
        &["--mapping", &mapping_arg, "fake"],
        &work,
        &envs_red,
        2,
        true,
    );
    let doc = status_json(&work, "repo/patches");
    let entry = &doc["patches"][PATCH];
    assert_eq!(
        entry["pr"]["ci"], "failing",
        "stage 2: red CI not recorded: {entry}"
    );
    assert_eq!(
        entry["pr"]["failingChecks"],
        serde_json::json!(["cargo fmt"]),
        "stage 2: failing checks not recorded: {entry}"
    );
    assert_eq!(
        doc["log"].as_array().unwrap().len(),
        2,
        "stage 2: expected 2 log transitions, got {}",
        doc["log"]
    );
    run_expect(
        exe,
        &["--fail-on-red-ci", "--mapping", &mapping_arg, "fake"],
        &work,
        &envs_red,
        2,
        false,
    );
    let doc = status_json(&work, "repo/patches");
    assert_eq!(
        doc["log"].as_array().unwrap().len(),
        2,
        "stage 2: log grew on an unchanged red re-run: {}",
        doc["log"]
    );

    // stage 3: merged upstream -> retired.
    fs::write(
        &view,
        r#"{"state":"MERGED","isDraft":false,"url":"https://github.com/fakeorg/fakerepo/pull/99999","number":99999}"#,
    )
    .unwrap();
    run_expect(
        exe,
        &["--mapping", &mapping_arg, "fake"],
        &work,
        &envs_red,
        3,
        true,
    );
    let doc = status_json(&work, "repo/patches");
    let entry = &doc["patches"][PATCH];
    assert_eq!(
        entry["pr"]["state"], "merged",
        "stage 3: not merged: {entry}"
    );
    assert_eq!(entry["retired"], true, "stage 3: not retired: {entry}");
    assert_eq!(
        doc["log"].as_array().unwrap().len(),
        4,
        "stage 3: expected 4 log transitions, got {}",
        doc["log"]
    );

    // stage 4: re-run is idempotent (no duplicate transitions), and a merged
    // PR never counts as red even under --fail-on-red-ci.
    run_expect(
        exe,
        &["--fail-on-red-ci", "--mapping", &mapping_arg, "fake"],
        &work,
        &envs_red,
        4,
        true,
    );
    let doc = status_json(&work, "repo/patches");
    assert_eq!(
        doc["log"].as_array().unwrap().len(),
        4,
        "stage 4: log grew on a no-change re-run: {}",
        doc["log"]
    );

    closure_gate_stages(exe, &work, &envs[0].1);
}

/// Stages 5 and 6: a closureGates fork (same patch/dag shape, its own patch
/// dir + status file) exercising the preflight branch (RFC 0010 A3) that
/// otherwise runs only on a real --open against a real flake. Split out of
/// the lifecycle test to keep it within clippy's function-length budget.
fn closure_gate_stages(exe: &str, work: &std::path::Path, path: &str) {
    write_series(work, "gated/patches");
    let gated_mapping = work.join("mapping-gated.json");
    fs::write(&gated_mapping, mapping_json("gated", "gated/patches", true)).unwrap();
    let gated_arg = gated_mapping.display().to_string();

    // stage 5: a red closure gate aborts the PR-opening.
    let envs_red = [("PATH", path.to_owned()), ("NIX_GATE_EXIT", "1".to_owned())];
    run_expect(
        exe,
        &["--open", "--mapping", &gated_arg, "gated"],
        work,
        &envs_red,
        5,
        true,
    );
    let entry = &status_json(work, "gated/patches")["patches"][PATCH];
    assert!(
        entry["pr"].is_null(),
        "stage 5: PR opened despite a failed gate: {entry}"
    );

    // stage 6: a green gate proceeds to open and record the PR.
    let envs = [("PATH", path.to_owned())];
    run_expect(
        exe,
        &["--open", "--mapping", &gated_arg, "gated"],
        work,
        &envs,
        6,
        true,
    );
    let entry = &status_json(work, "gated/patches")["patches"][PATCH];
    assert_eq!(
        entry["pr"]["number"], 99999,
        "stage 6: PR not recorded after a green gate: {entry}"
    );
}
