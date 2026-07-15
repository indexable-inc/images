//! Hermetic lifecycle test for the branch no other check reaches: the --open
//! recording path and the merged->retired transition run ONLY after a real
//! upstream PR exists, so a bug there surfaces on first outward use,
//! orphaning an opened-but-untracked PR and inviting a duplicate on the next
//! run. gh, upstream-pr, and nix are stubbed, so the whole PR lifecycle runs
//! sandboxed with no network: open + record, merged -> retired, idempotent
//! re-run, and the closure-gate preflight (red aborts, green proceeds).

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

fn mapping_json(name: &str, patch_dir: &str, closure_gates: bool) -> String {
    format!(
        r#"[{{"name":"{name}","input":"{name}-src","url":"https://github.com/fakeorg/fakerepo.git",
  "patchDir":"{patch_dir}","autoUpdate":false,"closureGates":{closure_gates},
  "upstreamPolicy":{{"prsWelcome":true,"aiPrsAllowed":"unknown","citation":"https://example.com","notes":"t"}},
  "patches":{{"{PATCH}":{{"upstream":"attempt","reason":"lifecycle test"}}}}}}]"#
    )
}

#[test]
fn pr_lifecycle_open_retire_idempotent_and_gates() {
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

    // stage 1: --open records the created PR.
    let run = run_bin(exe, &["--open", "--mapping", &mapping_arg, "fake"], &work, &envs);
    assert_eq!(run.status, 0, "stage 1 failed:\n{}\n{}", run.stdout, run.stderr);
    let entry = &status_json(&work, "repo/patches")["patches"][PATCH];
    assert_eq!(entry["pr"]["number"], 99999, "stage 1: PR not recorded: {entry}");
    assert_eq!(entry["pr"]["state"], "draft", "stage 1: PR not draft: {entry}");
    assert_eq!(entry["retired"], false, "stage 1: retired early: {entry}");

    // stage 2: merged upstream -> retired.
    let view = work.join("pr-view.json");
    fs::write(
        &view,
        r#"{"state":"MERGED","isDraft":false,"url":"https://github.com/fakeorg/fakerepo/pull/99999","number":99999}"#,
    )
    .unwrap();
    let envs_merged = [
        ("PATH", envs[0].1.clone()),
        ("GH_PR_VIEW_RESPONSE", view.display().to_string()),
    ];
    let run = run_bin(exe, &["--mapping", &mapping_arg, "fake"], &work, &envs_merged);
    assert_eq!(run.status, 0, "stage 2 failed:\n{}\n{}", run.stdout, run.stderr);
    let doc = status_json(&work, "repo/patches");
    let entry = &doc["patches"][PATCH];
    assert_eq!(entry["pr"]["state"], "merged", "stage 2: not merged: {entry}");
    assert_eq!(entry["retired"], true, "stage 2: not retired: {entry}");
    assert_eq!(doc["log"].as_array().unwrap().len(), 3, "stage 2: expected 3 log transitions, got {}", doc["log"]);

    // stage 3: re-run is idempotent (no duplicate transitions).
    let run = run_bin(exe, &["--mapping", &mapping_arg, "fake"], &work, &envs_merged);
    assert_eq!(run.status, 0, "stage 3 failed:\n{}\n{}", run.stdout, run.stderr);
    let doc = status_json(&work, "repo/patches");
    assert_eq!(doc["log"].as_array().unwrap().len(), 3, "stage 3: log grew on a no-change re-run: {}", doc["log"]);

    // A closureGates fork: same patch/dag shape, its own patch dir + status
    // file, exercising the preflight branch (RFC 0010 A3) that otherwise
    // runs only on a real --open against a real flake.
    write_series(&work, "gated/patches");
    let gated_mapping = work.join("mapping-gated.json");
    fs::write(&gated_mapping, mapping_json("gated", "gated/patches", true)).unwrap();
    let gated_arg = gated_mapping.display().to_string();

    // stage 4: a red closure gate aborts the PR-opening.
    let envs_red = [("PATH", envs[0].1.clone()), ("NIX_GATE_EXIT", "1".to_owned())];
    let run = run_bin(exe, &["--open", "--mapping", &gated_arg, "gated"], &work, &envs_red);
    assert_eq!(run.status, 0, "stage 4 failed:\n{}\n{}", run.stdout, run.stderr);
    let entry = &status_json(&work, "gated/patches")["patches"][PATCH];
    assert!(entry["pr"].is_null(), "stage 4: PR opened despite a failed gate: {entry}");

    // stage 5: a green gate proceeds to open and record the PR.
    let run = run_bin(exe, &["--open", "--mapping", &gated_arg, "gated"], &work, &envs);
    assert_eq!(run.status, 0, "stage 5 failed:\n{}\n{}", run.stdout, run.stderr);
    let entry = &status_json(&work, "gated/patches")["patches"][PATCH];
    assert_eq!(entry["pr"]["number"], 99999, "stage 5: PR not recorded after a green gate: {entry}");
}
