//! Regression test for ENG-11646: under the merge-forward doctrine the
//! branch is never rebased, so upstream is merged into it, and an earlier
//! revision of a patch gets merged in too when a flake.lock still pins it.
//! A series read over everything reachable from the branch then sees the
//! merge commits as patches and dies on the duplicate subject of two
//! revisions of one patch; the series is the branch's own first-parent line.
//!
//! The home-manager fork hit this for real: three revisions of one activation
//! patch, two sharing a subject, made `upstream-sync home-manager` fail with
//! "duplicate commit subject in the series".

mod common;

use std::fs;
use std::path::Path;

use common::{BODY, Fixture, mapping_json, run_bin};

const PATCH_ONE: &str = "fakefix: repair the frobnicator widget alignment";
const PATCH_TWO: &str = "fakefix: teach the widget to self-align";

fn run_sync(mapping: &Path, work: &Path, envs: &[(&str, String)]) -> common::Run {
    let exe = env!("CARGO_BIN_EXE_upstream-sync");
    let mapping = mapping.display().to_string();
    run_bin(exe, &["--dry-run", "--mapping", &mapping, "fake"], work, envs)
}

#[test]
fn the_series_is_the_branchs_own_line_not_everything_merged_into_it() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    let fixture = Fixture::merge_forwarded(root, &[(PATCH_ONE, BODY), (PATCH_TWO, BODY)]);
    let work = root.join("work");
    fs::create_dir(&work).unwrap();
    let envs = fixture.envs();

    let mapping = work.join("mapping.json");
    fs::write(&mapping, mapping_json("fake", "{}")).unwrap();
    let run = run_sync(&mapping, &work, &envs);

    assert_eq!(
        run.status, 0,
        "a merge-forwarded branch should read as its own two patches:\n{}\n{}",
        run.stdout, run.stderr
    );
    assert!(
        !run.stderr.contains("duplicate commit subject"),
        "the earlier revision merged in for ancestry was read as a second patch:\n{}",
        run.stderr
    );
    for subject in [PATCH_ONE, PATCH_TWO] {
        assert!(
            run.stdout.contains(subject),
            "series is missing '{subject}':\n{}",
            run.stdout
        );
    }
    // The merge commits are history, not patches. Naming one as a patch would
    // have the tool open an upstream PR titled "Merge upstream main".
    for merge in ["Merge upstream main", "Merge the revision a lock still pins"] {
        assert!(
            !run.stdout.contains(merge),
            "series leaked the merge commit '{merge}':\n{}",
            run.stdout
        );
    }
    assert!(
        run.stdout.contains("2 patch decisions"),
        "expected exactly the two patches:\n{}",
        run.stdout
    );
}

/// The other half of the shape detection, and the regression that a
/// `--first-parent --no-merges` read applied to every fork actually caused:
/// seven of fourteen forks lost their patches, because on a megamerge branch
/// the seal has one parent per patch and a patch may itself be a merge. This
/// asserts the sealed read still finds a patch reachable only as a second
/// parent, and one that is a merge commit.
#[test]
fn a_megamerge_seals_parents_are_all_still_patches() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    let fixture = Fixture::megamerge_dag(root, (PATCH_ONE, BODY), (PATCH_TWO, BODY));
    let work = root.join("work");
    fs::create_dir(&work).unwrap();
    let envs = fixture.envs();

    let mapping = work.join("mapping.json");
    fs::write(&mapping, mapping_json("fake", "{}")).unwrap();
    let run = run_sync(&mapping, &work, &envs);

    assert_eq!(
        run.status, 0,
        "sealed series read failed:\n{}\n{}",
        run.stdout, run.stderr
    );
    for subject in [PATCH_ONE, PATCH_TWO] {
        assert!(
            run.stdout.contains(subject),
            "sealed series dropped '{subject}', which is reachable only as a second parent:\n{}",
            run.stdout
        );
    }
    assert!(
        !run.stdout.contains("ix megamerge"),
        "the seal was read as a patch:\n{}",
        run.stdout
    );
    assert!(
        run.stdout.contains("2 patch decisions"),
        "expected both patches:\n{}",
        run.stdout
    );
}
