//! End-to-end `upstream-pr --dry-run` against a real local git upstream: the
//! closure/fetch/am/branch mechanism runs for real (real `git` on PATH via
//! the workspace's packageTestInputs); only push + PR are out of scope by
//! design of --dry-run. The nu predecessor never had this coverage.

mod common;

use std::fs;
use std::path::Path;
use std::process::Command;

use common::run_bin;

fn git(cwd: &Path, args: &[&str]) {
    let out = Command::new("git")
        .args(args)
        .current_dir(cwd)
        .env("GIT_AUTHOR_NAME", "Test")
        .env("GIT_AUTHOR_EMAIL", "t@t")
        .env("GIT_COMMITTER_NAME", "Test")
        .env("GIT_COMMITTER_EMAIL", "t@t")
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "git {args:?} failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
}

#[test]
fn dry_run_applies_closure_onto_upstream_tip() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();

    // A local "upstream" whose default branch is main, plus one commit on
    // top exported as our fork patch (then dropped from upstream, so the
    // patch is genuinely ours).
    let upstream = root.join("upstream");
    fs::create_dir(&upstream).unwrap();
    git(&upstream, &["init", "--quiet", "-b", "main"]);
    fs::write(upstream.join("README"), "hello\n").unwrap();
    git(&upstream, &["add", "."]);
    git(&upstream, &["commit", "--quiet", "-m", "base"]);
    fs::write(upstream.join("README"), "hello widget\n").unwrap();
    git(&upstream, &["add", "."]);
    git(
        &upstream,
        &[
            "commit",
            "--quiet",
            "-m",
            "fakefix: repair widget\n\nThis explains why.",
        ],
    );
    let patches = root.join("patches");
    fs::create_dir(&patches).unwrap();
    git(
        &upstream,
        &["format-patch", "-1", "-o", &patches.display().to_string()],
    );
    git(&upstream, &["reset", "--quiet", "--hard", "HEAD~1"]);
    let patch_name = fs::read_dir(&patches)
        .unwrap()
        .next()
        .unwrap()
        .unwrap()
        .file_name()
        .into_string()
        .unwrap();

    fs::write(
        patches.join("dag.json"),
        format!(r#"{{"comment":"t","base":"x","nodes":[{{"patch":"{patch_name}","deps":[]}}]}}"#),
    )
    .unwrap();
    let mapping = root.join("mapping.json");
    fs::write(
        &mapping,
        format!(
            r#"[{{"name":"fake","input":"fake-src","url":"file://{}","patchDir":"patches","autoUpdate":false,"patches":{{}}}}]"#,
            upstream.display()
        ),
    )
    .unwrap();

    let envs = [
        ("PATH", std::env::var("PATH").unwrap()),
        ("GIT_COMMITTER_NAME", "Test".to_owned()),
        ("GIT_COMMITTER_EMAIL", "t@t".to_owned()),
    ];
    let run = run_bin(
        env!("CARGO_BIN_EXE_upstream-pr"),
        &[
            "--dry-run",
            "--mapping",
            &mapping.display().to_string(),
            "fake",
            &patch_name,
        ],
        root,
        &envs,
    );
    assert_eq!(
        run.status, 0,
        "dry run failed:\n{}\n{}",
        run.stdout, run.stderr
    );
    assert!(
        run.stdout.contains("upstream default branch is main"),
        "{}",
        run.stdout
    );
    assert!(
        run.stdout.contains("applied 1 commit(s) cleanly"),
        "{}",
        run.stdout
    );
    assert!(
        run.stdout
            .contains("--dry-run: would push branch upstream-pr/fake/fakefix-repair-widget"),
        "{}",
        run.stdout
    );

    // The scratch repo named in the output survives for inspection.
    let scratch = run
        .stdout
        .lines()
        .find_map(|l| l.strip_prefix("upstream-pr: scratch repo left for inspection: "))
        .unwrap()
        .to_owned();
    assert!(
        Path::new(&scratch).join(".git").exists(),
        "scratch repo missing: {scratch}"
    );
    fs::remove_dir_all(&scratch).unwrap();
}
