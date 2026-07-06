//! End-to-end: apply twice (execute, then all cache hits), change one input,
//! verify only dependents invalidate, and render the HTML report.

use std::path::Path;
use std::process::Command;

// Inside this crate's directory: cargo-unit scopes each workspace crate's
// units to their own package root (lib/rust/cargo-unit.nix), so a fixture
// outside `packages/efx/cli/` would not exist in the nix build sandbox.
const SITE: &str = include_str!("../examples/site.efx");

struct Run {
    stdout: String,
    success: bool,
}

fn efx(dir: &Path, args: &[&str]) -> Run {
    let output = Command::new(env!("CARGO_BIN_EXE_efx"))
        .args(args)
        .current_dir(dir)
        .output()
        .expect("efx binary runs");
    Run {
        stdout: String::from_utf8_lossy(&output.stdout).into_owned(),
        success: output.status.success(),
    }
}

fn count(haystack: &str, needle: &str) -> usize {
    haystack.matches(needle).count()
}

#[test]
fn demo_scenario_executes_caches_and_invalidates() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("site.efx"), SITE).unwrap();

    // Run 1: nothing journaled, all three effects execute.
    let first = efx(dir.path(), &["apply", "site.efx"]);
    assert!(first.success, "{}", first.stdout);
    assert!(
        first.stdout.contains("3 executed, 0 cached"),
        "{}",
        first.stdout
    );
    let page = dir.path().join("out/index.html");
    let html = std::fs::read_to_string(&page).unwrap();
    assert!(html.contains("hello from efx"));
    assert!(
        html.contains("built by the efx demo"),
        "command output flowed in"
    );

    // Run 2: identical plan, the journal answers everything.
    let second = efx(dir.path(), &["apply", "site.efx"]);
    assert!(second.success, "{}", second.stdout);
    assert!(
        second.stdout.contains("0 executed, 3 cached"),
        "{}",
        second.stdout
    );

    // Change one let: the template literal changes, so `page` and its
    // dependent `site` invalidate while `stamp` stays cached.
    std::fs::write(
        dir.path().join("site.efx"),
        SITE.replace("hello from efx", "hello again"),
    )
    .unwrap();
    let plan = efx(dir.path(), &["plan", "site.efx"]);
    assert!(plan.success, "{}", plan.stdout);
    assert!(
        plan.stdout.contains("2 to execute, 1 cached"),
        "{}",
        plan.stdout
    );
    assert!(plan.stdout.contains("cached   stamp"), "{}", plan.stdout);
    assert!(
        plan.stdout.contains("input `template` changed"),
        "{}",
        plan.stdout
    );
    assert!(
        plan.stdout.contains("upstream `page` changed"),
        "{}",
        plan.stdout
    );

    let third = efx(dir.path(), &["apply", "site.efx"]);
    assert!(third.success, "{}", third.stdout);
    assert!(
        third.stdout.contains("2 executed, 1 cached"),
        "{}",
        third.stdout
    );
    assert!(
        std::fs::read_to_string(&page)
            .unwrap()
            .contains("hello again")
    );

    // The report shows all three runs, self-contained.
    let report_path = dir.path().join("report.html");
    let report = efx(
        dir.path(),
        &["report", "--html", report_path.to_str().unwrap()],
    );
    assert!(report.success, "{}", report.stdout);
    let rendered = std::fs::read_to_string(&report_path).unwrap();
    assert_eq!(count(&rendered, "<section class=\"run\">"), 3);
    assert!(rendered.contains("run 3"));
    assert!(rendered.contains("upstream `page` changed"));
    assert!(!rendered.contains("http://"), "no external assets");
    assert!(!rendered.contains("https://"), "no external assets");
}

#[test]
fn parse_errors_carry_location_and_source_line() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(
        dir.path().join("bad.efx"),
        "effect a \"cmd.run\" {\n  oops\n}\n",
    )
    .unwrap();
    let output = Command::new(env!("CARGO_BIN_EXE_efx"))
        .args(["plan", "bad.efx"])
        .current_dir(dir.path())
        .output()
        .expect("efx binary runs");
    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("bad.efx: 3:1"), "{stderr}");
    assert!(stderr.contains('^'), "caret rendering: {stderr}");
}
