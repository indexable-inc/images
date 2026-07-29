//! Drift-report test for the pure parts no live run pins deterministically:
//! flake.lock rev extraction, stance + retired counting, the next-action
//! heuristic, and both halves of the forge-failure contract. gh is stubbed
//! with fixed `--jq`'d responses; nothing elaborate, the forge is not what is
//! under test. Drift is deliberately clone-free, so no git fixture is needed
//! either.
//!
//! The two halves are the point, and they used to be one (ENG-11160): a forge
//! that ANSWERS 404 degrades to an unknown cell and a zero exit, because the
//! report must survive one bad pin and still render its other rows. A forge
//! that cannot be reached or refuses the read is fatal, because a drift table
//! computed from no information does not read as "unknown", it reads as "no
//! drift" -- and silent success is the failure mode this report exists to
//! prevent.

mod common;

use std::fs;

use common::{run_bin, stub_path, write_stub};

// `gh api <path> --jq <expr>` keyed on the path. Every bad-fork endpoint
// answers 404 the way gh really phrases it, exercising degrade-to-unknown.
const GH_STUB: &str = r#"case "$2" in
  repos/fakeorg/fakerepo) echo "main" ;;
  repos/fakeorg/fakerepo/compare/*) echo "123" ;;
  repos/fakeorg/fakerepo/commits/*) echo "2026-01-01T00:00:00Z" ;;
  *) echo "gh: Not Found (HTTP 404)" >&2; exit 1 ;;
esac"#;

// The same forge, unreachable rather than answering. Verbatim gh output for
// an expired token -- the exact failure the `notify` step hit in run
// 30443012384 while this probe was quietly reporting unknown cells.
const GH_STUB_UNREACHABLE: &str = r#"echo "HTTP 401: Bad credentials (https://api.github.com/graphql)" >&2
exit 1"#;

const MAPPING: &str = r#"[{"name":"fake","input":"fake-src","forkRepo":"fakefork/fakerepo",
  "upstreamUrl":"https://github.com/fakeorg/fakerepo.git","autoUpdate":false,
  "patches":{"fakefix: sent upstream":{"upstream":"attempt","reason":"t"},
             "fakefix: kept forever":{"upstream":"never","reason":"t"},
             "fakefix: wants polish":{"upstream":"hold","reason":"t"}}},
 {"name":"bad","input":"bad-src","forkRepo":"fakefork/badrepo",
  "upstreamUrl":"https://github.com/badorg/badrepo.git","autoUpdate":false,"patches":{}}]"#;

const FLAKE_LOCK: &str = r#"{"nodes": {"fake-src": {"locked": {"rev": "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"}},
           "bad-src": {"locked": {"rev": "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb"}}}}"#;

const FAKE_STATUS: &str = r#"{"comment":"t","lastChecked":"2026-01-01T00:00:00Z","patches":{"fakefix: sent upstream":{"upstream":"attempt","pr":{"url":"u","number":1,"state":"merged","checkedAt":"t"},"retired":true,"duplicates":[]}},"log":[]}"#;

#[test]
fn drift_json_and_markdown_surfaces() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    let stubs = root.join("stubs");
    fs::create_dir(&stubs).unwrap();
    write_stub(&stubs, "gh", GH_STUB);
    let work = root.join("work");
    fs::create_dir_all(work.join("packages/upstream-sync/status")).unwrap();
    fs::write(work.join("flake.lock"), FLAKE_LOCK).unwrap();
    fs::write(
        work.join("packages/upstream-sync/status/fake.json"),
        FAKE_STATUS,
    )
    .unwrap();
    let mapping = work.join("mapping.json");
    fs::write(&mapping, MAPPING).unwrap();
    let mapping_arg = mapping.display().to_string();
    let envs = [("PATH", stub_path(&stubs))];
    let exe = env!("CARGO_BIN_EXE_upstream-sync");

    // --json is the machine surface: stdout must parse as JSON alone
    // (warnings go to stderr), the fake row carries the stubbed forge facts
    // plus the retired-driven action, and the bad row (gh failing) is
    // unknown, not fatal.
    let run = run_bin(
        exe,
        &["drift", "--json", "--mapping", &mapping_arg],
        &work,
        &envs,
    );
    assert_eq!(
        run.status, 0,
        "drift --json failed:\n{}\n{}",
        run.stdout, run.stderr
    );
    let rows: serde_json::Value = serde_json::from_str(&run.stdout).unwrap();
    let rows = rows.as_array().unwrap();
    let fake = rows.iter().find(|r| r["name"] == "fake").unwrap();
    assert_eq!(
        fake["rev"], "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
        "fake drift facts: {fake}"
    );
    assert_eq!(fake["behind"], 123, "fake drift facts: {fake}");
    assert!(
        fake["ageDays"].as_i64().unwrap() >= 1,
        "fake drift facts: {fake}"
    );
    assert_eq!(fake["attempt"], 1, "fake stance counts: {fake}");
    assert_eq!(fake["hold"], 1, "fake stance counts: {fake}");
    assert_eq!(fake["never"], 1, "fake stance counts: {fake}");
    assert_eq!(fake["retired"], 1, "fake stance counts: {fake}");
    assert_eq!(
        fake["action"], "rebase-shrinks-series",
        "fake action: {fake}"
    );
    let bad = rows.iter().find(|r| r["name"] == "bad").unwrap();
    assert!(
        bad["behind"].is_null(),
        "bad row should degrade to unknown: {bad}"
    );
    assert!(
        bad["ageDays"].is_null(),
        "bad row should degrade to unknown: {bad}"
    );
    assert_eq!(
        bad["action"], "unknown",
        "bad row should degrade to unknown: {bad}"
    );

    // The markdown surface renders every fork as a table row, "?" for
    // unknowns.
    let run = run_bin(
        exe,
        &["drift", "--markdown", "--mapping", &mapping_arg],
        &work,
        &envs,
    );
    assert_eq!(
        run.status, 0,
        "drift --markdown failed:\n{}\n{}",
        run.stdout, run.stderr
    );
    assert!(
        run.stdout.contains("| fake"),
        "markdown table missing rows: {}",
        run.stdout
    );
    assert!(
        run.stdout.contains("| bad"),
        "markdown table missing rows: {}",
        run.stdout
    );
    assert!(
        run.stdout.contains('?'),
        "markdown table missing rows: {}",
        run.stdout
    );

    // --json and --markdown are mutually exclusive.
    let run = run_bin(
        exe,
        &["drift", "--json", "--markdown", "--mapping", &mapping_arg],
        &work,
        &envs,
    );
    assert_ne!(run.status, 0, "conflicting flags should fail");
}

/// The other half of the contract: a forge that never answers must abort the
/// report, and must say which fork and why.
///
/// Without this the degrade path swallows a bad token and prints a table of
/// question marks, which an operator reads as "we checked, nothing has
/// drifted". ENG-11160 spent four days in a nearby version of that state.
#[test]
fn an_unreachable_forge_aborts_instead_of_reporting_unknown() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    let stubs = root.join("stubs");
    fs::create_dir(&stubs).unwrap();
    write_stub(&stubs, "gh", GH_STUB_UNREACHABLE);
    let work = root.join("work");
    fs::create_dir_all(work.join("packages/upstream-sync/status")).unwrap();
    fs::write(work.join("flake.lock"), FLAKE_LOCK).unwrap();
    let mapping = work.join("mapping.json");
    fs::write(&mapping, MAPPING).unwrap();
    let mapping_arg = mapping.display().to_string();
    let envs = [("PATH", stub_path(&stubs))];
    let exe = env!("CARGO_BIN_EXE_upstream-sync");

    let run = run_bin(
        exe,
        &["drift", "--json", "--mapping", &mapping_arg],
        &work,
        &envs,
    );
    assert_ne!(
        run.status, 0,
        "an unreachable forge must fail the report, not fill it with unknowns:\n{}",
        run.stdout
    );
    // Naming the fork is the whole point: the operator has to know which
    // upstream to go and look at.
    assert!(
        run.stderr.contains("fake"),
        "the abort must name the fork it was probing: {}",
        run.stderr
    );
    assert!(
        run.stderr.contains("Bad credentials"),
        "the abort must carry the forge's own reason: {}",
        run.stderr
    );
}
