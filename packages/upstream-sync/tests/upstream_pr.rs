//! End-to-end `upstream-pr` against REAL local git repos in the megamerge
//! layout: series read, subject resolution, closure reporting, the
//! reason-of-record refusal, and the prepare-only branch push all run for
//! real (real `git` on PATH via the workspace's packageTestInputs); only
//! `gh pr create` is out of scope (behind --open, which the non-GitHub test
//! exercises via its early bail).

mod common;

use std::fs;

use common::{BODY, Fixture, SUBJECT, mapping_json, run_bin};

fn base_envs(fixture: &Fixture) -> Vec<(&'static str, String)> {
    let mut envs = fixture.envs();
    envs.push(("PATH", std::env::var("PATH").unwrap()));
    envs
}

#[test]
fn dry_run_resolves_closure_and_pushes_nothing() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    // Two chained patches: the second's contribution closure drags the
    // first (its git parent), which the tool must warn about.
    let fixture = Fixture::new(
        root,
        &[(SUBJECT, BODY), ("fakefix: also polish the widget", "Why.")],
    );
    let mapping = root.join("mapping.json");
    fs::write(&mapping, mapping_json("fake", "{}")).unwrap();

    let run = run_bin(
        env!("CARGO_BIN_EXE_upstream-pr"),
        &[
            "--dry-run",
            "--mapping",
            &mapping.display().to_string(),
            "fake",
            "polish",
        ],
        root,
        &base_envs(&fixture),
    );
    assert_eq!(
        run.status, 0,
        "dry run failed:\n{}\n{}",
        run.stdout, run.stderr
    );
    assert!(
        run.stdout
            .contains("target patch 'fakefix: also polish the widget'"),
        "{}",
        run.stdout
    );
    assert!(
        run.stdout.contains("drags 1 ancestor patch(es)"),
        "{}",
        run.stdout
    );
    assert!(
        run.stdout.contains("would push 2 commit(s)"),
        "{}",
        run.stdout
    );
    // --dry-run must leave the fork repo untouched.
    let branches = common::git(&fixture.fork, &["branch", "--list", "upstream/*"]);
    assert!(branches.is_empty(), "dry run pushed a branch: {branches}");
}

#[test]
fn prepare_pushes_the_patch_commit_to_the_fork_branch() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    let fixture = Fixture::new(root, &[(SUBJECT, BODY)]);
    let mapping = root.join("mapping.json");
    fs::write(&mapping, mapping_json("fake", "{}")).unwrap();

    let run = run_bin(
        env!("CARGO_BIN_EXE_upstream-pr"),
        &[
            "--mapping",
            &mapping.display().to_string(),
            "fake",
            "frobnicator",
        ],
        root,
        &base_envs(&fixture),
    );
    assert_eq!(
        run.status, 0,
        "prepare failed:\n{}\n{}",
        run.stdout, run.stderr
    );
    // The branch on the fork repo points at the patch commit itself: its
    // ancestry is the contribution.
    let branch = "upstream/fakefix-repair-the-frobnicator-widget-alignment";
    let pushed = common::git(&fixture.fork, &["rev-parse", &format!("refs/heads/{branch}")]);
    assert_eq!(pushed, fixture.sha_of(SUBJECT), "{}", run.stdout);
    assert!(
        run.stdout.contains(&format!(
            "compare/main...fakefork:fakerepo:{branch}?expand=1"
        )),
        "{}",
        run.stdout
    );
    assert!(run.stdout.contains("prepare-only"), "{}", run.stdout);
}

/// The reason-of-record refusal runs before ANY push, in every mode: a
/// body-less commit has nothing to say upstream.
#[test]
fn bodyless_commit_is_refused_before_any_push() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    let fixture = Fixture::new(root, &[("fakefix: no reason given", "")]);
    let mapping = root.join("mapping.json");
    fs::write(&mapping, mapping_json("fake", "{}")).unwrap();

    for mode in [&["--dry-run"][..], &[][..]] {
        let mut args = mode.to_vec();
        let mapping_arg = mapping.display().to_string();
        args.extend(["--mapping", &mapping_arg, "fake", "no reason"]);
        let run = run_bin(
            env!("CARGO_BIN_EXE_upstream-pr"),
            &args,
            root,
            &base_envs(&fixture),
        );
        assert_ne!(run.status, 0, "mode {mode:?} accepted a body-less commit");
        let combined = format!("{}{}", run.stdout, run.stderr);
        assert!(
            combined.contains("no commit-message body"),
            "mode {mode:?}: {combined}"
        );
        let branches = common::git(&fixture.fork, &["branch", "--list", "upstream/*"]);
        assert!(
            branches.is_empty(),
            "mode {mode:?} pushed despite the refusal: {branches}"
        );
    }
}

/// `--open` is the gh path; a non-GitHub upstream (mesa on gitlab) has none,
/// so the outward act bails up front instead of preparing a branch nothing
/// can use.
#[test]
fn open_against_non_github_upstream_bails() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    let mapping = root.join("mapping.json");
    fs::write(
        &mapping,
        r#"[{"name":"fake","input":"fake-src","forkRepo":"fakefork/fakerepo",
  "upstreamUrl":"https://gitlab.example.org/fakeorg/fakerepo.git","patches":{}}]"#,
    )
    .unwrap();
    let run = run_bin(
        env!("CARGO_BIN_EXE_upstream-pr"),
        &[
            "--open",
            "--mapping",
            &mapping.display().to_string(),
            "fake",
            "anything",
        ],
        root,
        &[("PATH", std::env::var("PATH").unwrap())],
    );
    assert_ne!(run.status, 0);
    let combined = format!("{}{}", run.stdout, run.stderr);
    assert!(combined.contains("not GitHub"), "{combined}");
}
