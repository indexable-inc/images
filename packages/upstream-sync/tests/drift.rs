//! Drift-report test for the pure parts no live run pins deterministically:
//! flake.lock rev extraction, stance + retired counting, the next-action
//! heuristic, and the degrade path (a failing forge yields unknown cells and
//! a zero exit, never a crashed report). gh is stubbed with fixed `--jq`'d
//! responses; nothing elaborate, the forge is not what is under test.

mod common;

use std::fs;

use common::{run_bin, stub_path, write_stub};

// `gh api <path> --jq <expr>` keyed on the path. Every bad-fork endpoint
// fails, exercising degrade-to-unknown.
const GH_STUB: &str = r#"case "$2" in
  repos/fakeorg/fakerepo) echo "main" ;;
  repos/fakeorg/fakerepo/compare/*) echo "123" ;;
  repos/fakeorg/fakerepo/commits/*) echo "2026-01-01T00:00:00Z" ;;
  *) echo "stub gh: unexpected: $*" >&2; exit 1 ;;
esac"#;

const MAPPING: &str = r#"[{"name":"fake","input":"fake-src","url":"https://github.com/fakeorg/fakerepo.git",
  "patchDir":"repo/patches","autoUpdate":false,
  "patches":{"0001-sent.patch":{"upstream":"attempt","reason":"t"},"0002-kept.patch":{"upstream":"never","reason":"t"}}},
 {"name":"bad","input":"bad-src","url":"https://github.com/badorg/badrepo.git",
  "patchDir":"bad/patches","autoUpdate":false,"patches":{}}]"#;

const FLAKE_LOCK: &str = r#"{"nodes": {"fake-src": {"locked": {"rev": "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"}},
           "bad-src": {"locked": {"rev": "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb"}}}}"#;

const FAKE_DAG: &str = r#"{"comment":"t","base":"deadbeef","nodes":[{"patch":"0001-sent.patch","deps":[]},{"patch":"0002-kept.patch","deps":[]},{"patch":"0003-unclassified.patch","deps":[]}]}"#;

const FAKE_STATUS: &str = r#"{"comment":"t","lastChecked":"2026-01-01T00:00:00Z","patches":{"0001-sent.patch":{"upstream":"attempt","pr":{"url":"u","number":1,"state":"merged","checkedAt":"t"},"retired":true,"duplicates":[]}},"log":[]}"#;

const BAD_DAG: &str = r#"{"comment":"t","base":"deadbeef","nodes":[{"patch":"0001-x.patch","deps":[]}]}"#;

#[test]
fn drift_json_and_markdown_surfaces() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    let stubs = root.join("stubs");
    fs::create_dir(&stubs).unwrap();
    write_stub(&stubs, "gh", GH_STUB);
    let work = root.join("work");
    fs::create_dir_all(work.join("repo/patches")).unwrap();
    fs::create_dir_all(work.join("bad/patches")).unwrap();
    fs::write(work.join("flake.lock"), FLAKE_LOCK).unwrap();
    fs::write(work.join("repo/patches/dag.json"), FAKE_DAG).unwrap();
    fs::write(work.join("repo/patches/upstream-status.json"), FAKE_STATUS).unwrap();
    fs::write(work.join("bad/patches/dag.json"), BAD_DAG).unwrap();
    let mapping = work.join("mapping.json");
    fs::write(&mapping, MAPPING).unwrap();
    let mapping_arg = mapping.display().to_string();
    let envs = [("PATH", stub_path(&stubs))];
    let exe = env!("CARGO_BIN_EXE_upstream-sync");

    // --json is the machine surface: stdout must parse as JSON alone
    // (warnings go to stderr), the fake row carries the stubbed forge facts
    // plus the retired-driven action, and the bad row (gh failing) is
    // unknown, not fatal.
    let run = run_bin(exe, &["drift", "--json", "--mapping", &mapping_arg], &work, &envs);
    assert_eq!(run.status, 0, "drift --json failed:\n{}\n{}", run.stdout, run.stderr);
    let rows: serde_json::Value = serde_json::from_str(&run.stdout).unwrap();
    let rows = rows.as_array().unwrap();
    let fake = rows.iter().find(|r| r["name"] == "fake").unwrap();
    assert_eq!(fake["rev"], "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa", "fake drift facts: {fake}");
    assert_eq!(fake["behind"], 123, "fake drift facts: {fake}");
    assert!(fake["ageDays"].as_i64().unwrap() >= 1, "fake drift facts: {fake}");
    assert_eq!(fake["attempt"], 1, "fake stance counts: {fake}");
    assert_eq!(fake["hold"], 1, "fake stance counts: {fake}");
    assert_eq!(fake["never"], 1, "fake stance counts: {fake}");
    assert_eq!(fake["retired"], 1, "fake stance counts: {fake}");
    assert_eq!(fake["action"], "rebase-shrinks-series", "fake action: {fake}");
    let bad = rows.iter().find(|r| r["name"] == "bad").unwrap();
    assert!(bad["behind"].is_null(), "bad row should degrade to unknown: {bad}");
    assert!(bad["ageDays"].is_null(), "bad row should degrade to unknown: {bad}");
    assert_eq!(bad["action"], "unknown", "bad row should degrade to unknown: {bad}");

    // The markdown surface renders every fork as a table row, "?" for
    // unknowns.
    let run = run_bin(exe, &["drift", "--markdown", "--mapping", &mapping_arg], &work, &envs);
    assert_eq!(run.status, 0, "drift --markdown failed:\n{}\n{}", run.stdout, run.stderr);
    assert!(run.stdout.contains("| fake"), "markdown table missing rows: {}", run.stdout);
    assert!(run.stdout.contains("| bad"), "markdown table missing rows: {}", run.stdout);
    assert!(run.stdout.contains('?'), "markdown table missing rows: {}", run.stdout);

    // --json and --markdown are mutually exclusive.
    let run = run_bin(exe, &["drift", "--json", "--markdown", "--mapping", &mapping_arg], &work, &envs);
    assert_ne!(run.status, 0, "conflicting flags should fail");
}
