//! Regression test for #4038: a fork based on an upstream MAINTENANCE
//! branch (nix's 2.34.7 base on 2.34-maintenance) while the upstream's
//! default branch has diverged. Anchoring the series base on the default
//! branch merge-bases at the fork point and drags the maintenance commits
//! into the series, dying on their duplicate "Bump version" subjects; the
//! registry's `upstreamRef` pins the anchor so the series is exactly the
//! fork's patches.

mod common;

use std::fs;
use std::path::Path;

use common::{Fixture, mapping_json_on, run_bin};

const PATCH_ONE: &str = "fakefix: repair the frobnicator widget alignment";
const PATCH_TWO: &str = "fakefix: teach the widget to self-align";

fn run_sync(mapping: &Path, work: &Path, envs: &[(&str, String)]) -> common::Run {
    let exe = env!("CARGO_BIN_EXE_upstream-sync");
    let mapping = mapping.display().to_string();
    run_bin(exe, &["--dry-run", "--mapping", &mapping, "fake"], work, envs)
}

#[test]
fn upstream_ref_anchors_the_series_base() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    let fixture = Fixture::on_maintenance_branch(
        root,
        &[(PATCH_ONE, common::BODY), (PATCH_TWO, common::BODY)],
    );
    let work = root.join("work");
    fs::create_dir(&work).unwrap();
    let envs = fixture.envs();

    // Without `upstreamRef` the walk anchors on the diverged default
    // branch: the undershot merge-base pulls the upstream maintenance
    // commits into the series and the read dies on their duplicate
    // subject. This guards the fixture itself: if it stops reproducing
    // #4038, the passing half below proves nothing.
    let mapping = work.join("mapping-default.json");
    fs::write(&mapping, mapping_json_on("fake", None, "{}")).unwrap();
    let run = run_sync(&mapping, &work, &envs);
    assert_ne!(
        run.status, 0,
        "default-branch anchoring should die on the duplicate maintenance subject:\n{}\n{}",
        run.stdout, run.stderr
    );
    assert!(
        run.stderr.contains("duplicate commit subject"),
        "expected the duplicate-subject error:\n{}\n{}",
        run.stdout,
        run.stderr
    );

    // With `upstreamRef` the base anchors on the branch the fork is
    // actually based on: the series is exactly the fork's patches.
    let mapping = work.join("mapping-ref.json");
    fs::write(&mapping, mapping_json_on("fake", Some("maintenance"), "{}")).unwrap();
    let run = run_sync(&mapping, &work, &envs);
    assert_eq!(
        run.status, 0,
        "upstreamRef-anchored read failed:\n{}\n{}",
        run.stdout, run.stderr
    );
    for subject in [PATCH_ONE, PATCH_TWO] {
        assert!(
            run.stdout.contains(subject),
            "series is missing '{subject}':\n{}",
            run.stdout
        );
    }
    assert!(
        !run.stdout.contains("Bump version"),
        "series leaked upstream maintenance commits:\n{}",
        run.stdout
    );
    assert!(
        run.stdout.contains("2 patch decisions"),
        "expected exactly the 2 fork patches:\n{}",
        run.stdout
    );
}
