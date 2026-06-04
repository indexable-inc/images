//! End-to-end checks that drive the built binary against a temporary git
//! repository created with `git2`, so the output reflects real repo queries.

use std::process::Command;

use git2::{Repository, Signature};
use tempfile::TempDir;

/// A repo whose first commit sits on `main`: the open repo, its temp dir (kept
/// alive for the test), and the initial `main` commit oid.
struct MainRepo {
    repo: Repository,
    dir: TempDir,
    main_oid: git2::Oid,
}

/// Initialize a [`MainRepo`]. `git init`'s default branch name varies, so HEAD is
/// forced to `refs/heads/main` before the first commit lands.
fn init_on_main() -> MainRepo {
    let dir = tempfile::tempdir().expect("tempdir");
    let repo = Repository::init(dir.path()).expect("init repo");
    repo.set_head("refs/heads/main").expect("point HEAD at main");

    let sig = signature();
    std::fs::write(dir.path().join("README.md"), "hello\n").unwrap();
    let main_oid = commit(&repo, &sig, &["README.md"], "chore: initial commit", &[]);

    MainRepo {
        repo,
        dir,
        main_oid,
    }
}

/// Build a repo with one commit on `main` and a feature commit checked out on a
/// branch that is one commit ahead.
fn repo_ahead_of_main() -> TempDir {
    let MainRepo {
        repo,
        dir,
        main_oid,
    } = init_on_main();
    let sig = signature();

    let main_commit = repo.find_commit(main_oid).unwrap();
    repo.branch("feature", &main_commit, false).expect("create feature");
    repo.set_head("refs/heads/feature").expect("checkout feature");

    std::fs::create_dir_all(dir.path().join("src")).unwrap();
    std::fs::write(dir.path().join("src/lib.rs"), "// code\n").unwrap();
    commit(&repo, &sig, &["src/lib.rs"], "feat(core): add lib", &[main_oid]);

    dir
}

/// A fixed test author so commits are deterministic.
fn signature() -> Signature<'static> {
    Signature::now("Tester", "tester@example.com").expect("signature")
}

/// Stage `paths`, write the tree, and create a commit with `parents`. Returns
/// the new commit oid and updates HEAD.
fn commit(
    repo: &Repository,
    sig: &Signature<'_>,
    paths: &[&str],
    message: &str,
    parents: &[git2::Oid],
) -> git2::Oid {
    let mut index = repo.index().unwrap();
    for path in paths {
        index.add_path(std::path::Path::new(path)).unwrap();
    }
    index.write().unwrap();
    let tree = repo.find_tree(index.write_tree().unwrap()).unwrap();

    let parent_commits: Vec<git2::Commit> =
        parents.iter().map(|oid| repo.find_commit(*oid).unwrap()).collect();
    let parent_refs: Vec<&git2::Commit> = parent_commits.iter().collect();

    repo.commit(Some("HEAD"), sig, sig, message, &tree, &parent_refs)
        .unwrap()
}

/// Run the binary in `cwd` with `args`, returning its captured output.
fn run(cwd: &std::path::Path, args: &[&str]) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_git-log-pretty"))
        .args(args)
        .current_dir(cwd)
        .output()
        .expect("spawn git-log-pretty")
}

/// Strip SGR sequences so assertions can match the visible text.
fn plain(bytes: &[u8]) -> String {
    String::from_utf8(strip_ansi_escapes::strip(bytes)).unwrap()
}

#[test]
fn log_lists_commits_ahead_of_main() {
    let dir = repo_ahead_of_main();
    let output = run(dir.path(), &[]);
    assert!(output.status.success(), "stderr: {}", plain(&output.stderr));

    let stdout = plain(&output.stdout);
    assert!(stdout.contains("commit ahead of main"), "got: {stdout}");
    assert!(stdout.contains("feat"), "missing chip type: {stdout}");
    assert!(stdout.contains("src/lib.rs"), "missing file tree: {stdout}");
}

#[test]
fn no_pager_flag_is_accepted_and_renders_log() {
    // The pager only engages on a TTY, which the test harness never is, so this
    // mainly guards that `--no-pager` parses and the writer path still renders.
    let dir = repo_ahead_of_main();
    let output = run(dir.path(), &["--no-pager"]);
    assert!(output.status.success(), "stderr: {}", plain(&output.stderr));

    let stdout = plain(&output.stdout);
    assert!(stdout.contains("commit ahead of main"), "got: {stdout}");
    assert!(stdout.contains("src/lib.rs"), "missing file tree: {stdout}");
}

#[test]
fn log_shows_recent_history_on_main() {
    // On `main` there is nothing to be ahead of, so the default view lists
    // main's own recent commits rather than reporting "caught up".
    let MainRepo { dir, .. } = init_on_main();

    let output = run(dir.path(), &[]);
    assert!(output.status.success(), "stderr: {}", plain(&output.stderr));

    let stdout = plain(&output.stdout);
    assert!(stdout.contains("Recent commits on main"), "got: {stdout}");
    assert!(stdout.contains("initial commit"), "missing commit: {stdout}");
}

#[test]
fn log_reports_caught_up_off_main_when_level_with_main() {
    // A non-main branch sitting exactly at main has no ahead commits, so the
    // caught-up message still applies there.
    let MainRepo {
        repo,
        dir,
        main_oid,
    } = init_on_main();
    let main_commit = repo.find_commit(main_oid).unwrap();
    repo.branch("feature", &main_commit, false).expect("create feature");
    repo.set_head("refs/heads/feature").expect("checkout feature");

    let output = run(dir.path(), &[]);
    assert!(output.status.success(), "stderr: {}", plain(&output.stderr));
    assert!(plain(&output.stdout).contains("All caught up with main"));
}

#[test]
fn diff_uses_merge_base_after_main_advances() {
    // main advances past the branch point. The branch only touched src/lib.rs,
    // so the `main...HEAD` diff must not include main-only files.
    let MainRepo {
        repo,
        dir,
        main_oid,
    } = init_on_main();
    let sig = signature();

    let main_commit = repo.find_commit(main_oid).unwrap();
    repo.branch("feature", &main_commit, false).expect("create feature");

    // One commit on main after the fork point, touching a file the branch never
    // sees.
    std::fs::write(dir.path().join("main-only.md"), "main\n").unwrap();
    commit(&repo, &sig, &["main-only.md"], "docs: main only", &[main_oid]);

    // The feature branch diverges with its own file. Reset the shared index to
    // the fork-point tree first; otherwise main-only.md staged above would leak
    // into the feature commit's tree.
    repo.set_head("refs/heads/feature").expect("checkout feature");
    let mut index = repo.index().unwrap();
    index.read_tree(&main_commit.tree().unwrap()).unwrap();
    index.write().unwrap();
    std::fs::create_dir_all(dir.path().join("src")).unwrap();
    std::fs::write(dir.path().join("src/lib.rs"), "// code\n").unwrap();
    commit(&repo, &sig, &["src/lib.rs"], "feat(core): add lib", &[main_oid]);

    let output = run(dir.path(), &["diff", "main", "HEAD"]);
    assert!(output.status.success(), "stderr: {}", plain(&output.stderr));

    let stdout = plain(&output.stdout);
    assert!(stdout.contains("src/lib.rs"), "missing branch file: {stdout}");
    assert!(!stdout.contains("main-only.md"), "leaked main-only file: {stdout}");
}

#[test]
fn diff_subcommand_renders_changed_file_tree() {
    let dir = repo_ahead_of_main();
    let output = run(dir.path(), &["diff", "main", "HEAD"]);
    assert!(output.status.success(), "stderr: {}", plain(&output.stderr));

    let stdout = plain(&output.stdout);
    assert!(stdout.contains("files changed in main...HEAD"), "got: {stdout}");
    assert!(stdout.contains("src/lib.rs"), "got: {stdout}");
}
