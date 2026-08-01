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
    // The earlier revision merged in for ancestry shares its subject with the
    // revision on the branch, so it is represented rather than lost. Reporting
    // it would fire on every fork of this shape and teach everyone to ignore
    // the warning that ENG-11686 exists to make audible.
    assert!(
        !run.stderr.contains("second parent"),
        "false positive: the benign ancestry merge was called an invisible patch:\n{}",
        run.stderr
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

/// ENG-11686: a patch that arrived as a merged pull request is on the merge's
/// second parent, so the branch's own line never reaches it. It was absent
/// from the series rather than merely unclassified, which is worse: an intent
/// entry naming it is an orphaned key that fails the run, so the gap could not
/// even be written down. On indexable-inc/nix that was 22 patches across 7
/// merged pull requests, and on indexable-inc/jj 11 across 5.
#[test]
fn a_patch_merged_as_a_pull_request_is_in_the_series() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    let fixture = Fixture::pr_merged(root, (PATCH_ONE, BODY), (PATCH_TWO, BODY));
    let work = root.join("work");
    fs::create_dir(&work).unwrap();
    let envs = fixture.envs();

    let mapping = work.join("mapping.json");
    fs::write(&mapping, mapping_json("fake", "{}")).unwrap();
    let run = run_sync(&mapping, &work, &envs);

    assert_eq!(
        run.status, 0,
        "recovering the merged patch should not fail the run:\n{}\n{}",
        run.stdout, run.stderr
    );
    assert!(
        run.stdout.contains(PATCH_TWO),
        "the patch that landed on the merge's second parent is still invisible:\n{}",
        run.stdout
    );
    assert!(
        run.stdout.contains("2 patch decisions"),
        "expected the branch's own patch and the merged one:\n{}",
        run.stdout
    );
    // The merge itself is history, not a patch: naming it as one would have
    // the tool offer "Merge pull request #1" upstream.
    assert!(
        !run.stdout.contains("Merge pull request"),
        "the merge commit was read as a patch:\n{}",
        run.stdout
    );
    // Recovered, but still against `forkBranches`; a silently absorbed merge
    // is one nobody stops producing.
    assert!(
        run.stderr.contains("forkBranches") && run.stderr.contains(PATCH_TWO),
        "recovering the patch should still name the shape that hid it:\n{}",
        run.stderr
    );
}

/// The fence the recovery must not tear down. A revision some flake.lock still
/// pins is merged back for ancestry alone, and on home-manager it was RETITLED
/// since, so its subject does not match the branch's copy. Only the merge's
/// tree says it carried no patch. Reading it as one is ENG-11646, which killed
/// `upstream-sync home-manager` on a duplicate subject; reading it as a
/// SEPARATE patch, which a subject filter alone would do here, silently offers
/// a stale revision of a patch upstream.
#[test]
fn a_retitled_revision_merged_for_ancestry_is_not_a_patch() {
    const OLD: &str = "files: batch symlink creation and target checks in activation";
    const NEW: &str = "files: batch link creation and target checks";

    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    let fixture = Fixture::retitled_pinned_revision(root, (OLD, BODY), (NEW, BODY));
    let work = root.join("work");
    fs::create_dir(&work).unwrap();
    let envs = fixture.envs();

    let mapping = work.join("mapping.json");
    fs::write(&mapping, mapping_json("fake", "{}")).unwrap();
    let run = run_sync(&mapping, &work, &envs);

    assert_eq!(
        run.status, 0,
        "an ancestry-only merge-back should read as no extra patch:\n{}\n{}",
        run.stdout, run.stderr
    );
    assert!(
        run.stdout.contains(NEW),
        "the branch's own patch is missing:\n{}",
        run.stdout
    );
    assert!(
        !run.stdout.contains(OLD),
        "the pinned earlier revision was read as a second patch:\n{}",
        run.stdout
    );
    assert!(
        run.stdout.contains("1 patch decision"),
        "expected exactly the branch's own patch:\n{}",
        run.stdout
    );
    // It is not a lost patch either, so it must not be reported as one.
    assert!(
        !run.stderr.contains(OLD),
        "the ancestry-only merge-back was reported as a patch hidden by a merge:\n{}",
        run.stderr
    );
}
