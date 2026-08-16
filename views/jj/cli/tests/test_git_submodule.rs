// Copyright 2026 The Jujutsu Authors
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
// https://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

use std::path::Path;
use std::path::PathBuf;

use indoc::indoc;
use jj_lib::object_id::ObjectId as _;
use jj_lib::ref_name::RefName;
use jj_lib::ref_name::RemoteName;
use jj_lib::repo::StoreFactories;
use jj_lib::submodule_store::SubmoduleName;
use jj_lib::submodule_store::encode_dir_name;
use jj_lib::submodule_store::load_submodule_repo;
use pollster::FutureExt as _;
use testutils::git;

use crate::common::TestEnvironment;
use crate::common::TestWorkDir;

/// Creates a Git repo with a single commit on `main`, to be used as the remote
/// of a submodule.
fn set_up_source(test_env: &TestEnvironment, name: &str) -> gix::ObjectId {
    let repo = git::init(test_env.env_root().join(name));
    let result = git::add_commit(
        &repo,
        "refs/heads/main",
        "file",
        name.as_bytes(),
        "message",
        &[],
    );
    git::set_symbolic_reference(&repo, "HEAD", "refs/heads/main");
    result.commit_id
}

/// Writes a `[submodule "<name>"]` section using the real `git` binary, which
/// is the same thing `git submodule add` does to `.gitmodules`.
fn declare_submodule(work_dir: &TestWorkDir, name: &str, path: &str, url: &str) {
    for (key, value) in [("path", path), ("url", url)] {
        work_dir
            .run_jj([
                "util",
                "exec",
                "--",
                "git",
                "config",
                "--file",
                ".gitmodules",
                &format!("submodule.{name}.{key}"),
                value,
            ])
            .success();
    }
}

fn submodule_repo_path(work_dir: &TestWorkDir, name: &str) -> PathBuf {
    let name = SubmoduleName::new(name).unwrap();
    work_dir
        .root()
        .join(".jj")
        .join("repo")
        .join("submodule_store")
        .join("repos")
        .join(encode_dir_name(&name).unwrap())
}

/// Asserts that `repo_path` holds a jj repo whose view has `main@origin` at
/// `expected`.
///
/// Reading the view rather than the Git refs is the point: the submodule is
/// stored as a full jj repo with its own operation log, so a clone that left
/// the objects in place but never imported them would pass a Git-level check
/// and still be useless to jj.
fn assert_submodule_repo_at(repo_path: &Path, expected: gix::ObjectId) {
    assert_eq!(
        std::fs::read_to_string(repo_path.join("store").join("type")).unwrap(),
        "git"
    );
    let settings = testutils::user_settings();
    let repo = load_submodule_repo(&settings, repo_path, &StoreFactories::default())
        .block_on()
        .unwrap();
    let symbol = RefName::new("main").to_remote_symbol(RemoteName::new("origin"));
    let target = repo.view().get_remote_bookmark(symbol).target.clone();
    assert_eq!(
        target.as_normal().map(|id| id.hex()),
        Some(expected.to_string())
    );
}

/// Runs `git` in `cwd` and returns its trimmed stdout.
///
/// The identity and both timestamps are pinned, because the submodule commit
/// ids these fixtures produce are printed by `jj git submodule status` and so
/// have to hash the same on every run.
fn run_git(cwd: &Path, args: &[&str]) -> String {
    let output = std::process::Command::new("git")
        .current_dir(cwd)
        .args(args)
        .env("GIT_AUTHOR_NAME", "Test User")
        .env("GIT_AUTHOR_EMAIL", "test.user@example.com")
        .env("GIT_AUTHOR_DATE", "2001-02-03T04:05:06+07:00")
        .env("GIT_COMMITTER_NAME", "Test User")
        .env("GIT_COMMITTER_EMAIL", "test.user@example.com")
        .env("GIT_COMMITTER_DATE", "2001-02-03T04:05:06+07:00")
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "git {args:?} failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout).unwrap().trim().to_owned()
}

/// Creates the Git repo a submodule is cloned from, with one commit in it, and
/// returns its path and that commit's id.
fn init_submodule_source(env_root: &Path, name: &str) -> (PathBuf, String) {
    let path = env_root.join(name);
    std::fs::create_dir(&path).unwrap();
    run_git(&path, &["init", "--quiet"]);
    std::fs::write(path.join("file"), format!("{name} first\n")).unwrap();
    run_git(&path, &["add", "file"]);
    run_git(&path, &["commit", "--quiet", "-m", "first"]);
    let commit_id = run_git(&path, &["rev-parse", "HEAD"]);
    (path, commit_id)
}

/// Adds a commit to a submodule's source repo and returns its id.
fn add_submodule_commit(source: &Path) -> String {
    std::fs::write(source.join("file"), "second\n").unwrap();
    run_git(source, &["add", "file"]);
    run_git(source, &["commit", "--quiet", "-m", "second"]);
    run_git(source, &["rev-parse", "HEAD"])
}

/// Copies `source` as it is right now.
///
/// A commit added to `source` afterwards is missing here, which is what a
/// submodule that has been cloned but not fetched since looks like.
fn clone_submodule_source(env_root: &Path, source: &Path, name: &str) -> PathBuf {
    let path = env_root.join(name);
    run_git(
        env_root,
        &[
            "clone",
            "--quiet",
            "--bare",
            source.to_str().unwrap(),
            path.to_str().unwrap(),
        ],
    );
    path
}

/// Builds the superproject's history with real `git` and returns a jj repo
/// backed by it.
///
/// jj cannot write a gitlink into a tree, so the commit under test is built
/// with `git write-tree`. It is left on the `subs` bookmark rather than on
/// `HEAD`, so that `jj git init` checks out an empty tree and the tests see
/// only the output of the command they are testing.
fn init_superproject<'a>(
    test_env: &'a TestEnvironment,
    gitmodules: Option<&str>,
    gitlinks: &[(&str, &str)],
) -> TestWorkDir<'a> {
    let origin = test_env.env_root().join("origin");
    std::fs::create_dir(&origin).unwrap();
    run_git(&origin, &["init", "--quiet"]);
    run_git(
        &origin,
        &["commit", "--quiet", "--allow-empty", "-m", "base"],
    );
    if let Some(gitmodules) = gitmodules {
        let blob_path = origin.join("gitmodules-blob");
        std::fs::write(&blob_path, gitmodules).unwrap();
        let blob = run_git(&origin, &["hash-object", "-w", "gitmodules-blob"]);
        std::fs::remove_file(&blob_path).unwrap();
        run_git(
            &origin,
            &[
                "update-index",
                "--add",
                "--cacheinfo",
                &format!("100644,{blob},.gitmodules"),
            ],
        );
    }
    for (path, commit_id) in gitlinks {
        run_git(
            &origin,
            &[
                "update-index",
                "--add",
                "--cacheinfo",
                &format!("160000,{commit_id},{path}"),
            ],
        );
    }
    let tree = run_git(&origin, &["write-tree"]);
    let commit = run_git(&origin, &["commit-tree", &tree, "-p", "HEAD", "-m", "subs"]);
    run_git(&origin, &["update-ref", "refs/heads/subs", &commit]);
    // Leave the index matching HEAD so nothing else sees the scratch entries.
    run_git(&origin, &["read-tree", "HEAD"]);

    test_env
        .run_jj_in(
            ".",
            [
                "git",
                "init",
                "--git-repo",
                origin.to_str().unwrap(),
                "repo",
            ],
        )
        .success();
    test_env.work_dir("repo")
}

/// Puts a repo for `name` in `work_dir`'s submodule store, backed by the Git
/// repo at `git_repo_path`.
///
/// `jj git submodule clone` builds the same thing, but always from the whole of
/// what the source has right now. These fixtures need a store whose contents
/// are pinned instead: a clone taken before a commit existed, or a repo no
/// revision declares. So this assembles an ordinary jj repo in the directory
/// the store hands out for that name.
fn add_submodule_repo(
    test_env: &TestEnvironment,
    work_dir: &TestWorkDir,
    name: &str,
    git_repo_path: &Path,
) {
    let scratch = test_env.env_root().join("submodule-scratch");
    test_env
        .run_jj_in(
            ".",
            [
                "git",
                "init",
                "--git-repo",
                git_repo_path.to_str().unwrap(),
                scratch.to_str().unwrap(),
            ],
        )
        .success();
    let repo_path = submodule_repo_path(work_dir, name);
    std::fs::create_dir_all(repo_path.parent().unwrap()).unwrap();
    std::fs::rename(scratch.join(".jj").join("repo"), repo_path).unwrap();
    std::fs::remove_dir_all(&scratch).unwrap();
}

#[test]
fn test_git_submodule_clone_named() {
    let test_env = TestEnvironment::default();
    set_up_source(&test_env, "source-a");
    set_up_source(&test_env, "source-b");
    test_env.run_jj_in(".", ["git", "init", "super"]).success();
    let work_dir = test_env.work_dir("super");
    declare_submodule(
        &work_dir,
        "a",
        "a",
        test_env.env_root().join("source-a").to_str().unwrap(),
    );
    declare_submodule(
        &work_dir,
        "b",
        "b",
        test_env.env_root().join("source-b").to_str().unwrap(),
    );

    let output = work_dir.run_jj(["git", "submodule", "clone", "a"]);
    insta::assert_snapshot!(output, @r#"
    ------- stderr -------
    Cloning submodule "a" from $TEST_ENV/source-a
    Cloned submodule "a": 1 bookmark, 0 tags.
    [EOF]
    "#);

    assert!(submodule_repo_path(&work_dir, "a").exists());
    assert!(!submodule_repo_path(&work_dir, "b").exists());
}

#[test]
fn test_git_submodule_clone_all_by_default() {
    let test_env = TestEnvironment::default();
    set_up_source(&test_env, "source-a");
    set_up_source(&test_env, "source-b");
    test_env.run_jj_in(".", ["git", "init", "super"]).success();
    let work_dir = test_env.work_dir("super");
    declare_submodule(
        &work_dir,
        "a",
        "a",
        test_env.env_root().join("source-a").to_str().unwrap(),
    );
    declare_submodule(
        &work_dir,
        "b",
        "b",
        test_env.env_root().join("source-b").to_str().unwrap(),
    );

    let output = work_dir.run_jj(["git", "submodule", "clone"]);
    insta::assert_snapshot!(output, @r#"
    ------- stderr -------
    Cloning submodule "a" from $TEST_ENV/source-a
    Cloned submodule "a": 1 bookmark, 0 tags.
    Cloning submodule "b" from $TEST_ENV/source-b
    Cloned submodule "b": 1 bookmark, 0 tags.
    [EOF]
    "#);

    assert!(submodule_repo_path(&work_dir, "a").exists());
    assert!(submodule_repo_path(&work_dir, "b").exists());
}

#[test]
fn test_git_submodule_clone_creates_jj_repo() {
    let test_env = TestEnvironment::default();
    let commit_id = set_up_source(&test_env, "source");
    test_env.run_jj_in(".", ["git", "init", "super"]).success();
    let work_dir = test_env.work_dir("super");
    declare_submodule(
        &work_dir,
        "sub",
        "sub",
        test_env.env_root().join("source").to_str().unwrap(),
    );
    work_dir.run_jj(["git", "submodule", "clone"]).success();

    // Hex of "sub", per the submodule store's directory naming.
    let repo_path = submodule_repo_path(&work_dir, "sub");
    assert!(repo_path.ends_with("737562"));
    assert_submodule_repo_at(&repo_path, commit_id);
}

#[test]
fn test_git_submodule_clone_no_gitmodules() {
    let test_env = TestEnvironment::default();
    test_env.run_jj_in(".", ["git", "init", "super"]).success();
    let work_dir = test_env.work_dir("super");

    let output = work_dir.run_jj(["git", "submodule", "clone"]);
    insta::assert_snapshot!(output, @"
    ------- stderr -------
    No submodules are declared in .gitmodules at the working-copy commit.
    [EOF]
    ");
}

#[test]
fn test_git_submodule_clone_undeclared_name() {
    let test_env = TestEnvironment::default();
    set_up_source(&test_env, "source-a");
    test_env.run_jj_in(".", ["git", "init", "super"]).success();
    let work_dir = test_env.work_dir("super");
    declare_submodule(
        &work_dir,
        "a",
        "a",
        test_env.env_root().join("source-a").to_str().unwrap(),
    );

    // Every name is resolved before anything is cloned, so the declared "a" is
    // left alone rather than half-cloning the request.
    let output = work_dir.run_jj(["git", "submodule", "clone", "a", "nope"]);
    insta::assert_snapshot!(output, @r#"
    ------- stderr -------
    Error: No submodule named "nope" in .gitmodules at the working-copy commit
    [EOF]
    [exit status: 1]
    "#);
    assert!(!submodule_repo_path(&work_dir, "a").exists());
}

#[test]
fn test_git_submodule_clone_already_in_store() {
    let test_env = TestEnvironment::default();
    set_up_source(&test_env, "source");
    test_env.run_jj_in(".", ["git", "init", "super"]).success();
    let work_dir = test_env.work_dir("super");
    declare_submodule(
        &work_dir,
        "sub",
        "sub",
        test_env.env_root().join("source").to_str().unwrap(),
    );
    work_dir.run_jj(["git", "submodule", "clone"]).success();

    // A second run reports the submodule instead of failing or fetching into
    // it. That keeps re-running the command after adding a submodule cheap.
    let output = work_dir.run_jj(["git", "submodule", "clone"]);
    insta::assert_snapshot!(output, @r#"
    ------- stderr -------
    Skipping submodule "sub": it already has a repo in the submodule store.
    Nothing changed.
    [EOF]
    "#);
}

#[test]
fn test_git_submodule_clone_relative_url() {
    let test_env = TestEnvironment::default();
    set_up_source(&test_env, "source-super");
    let commit_id = set_up_source(&test_env, "source-sub");
    test_env
        .run_jj_in(".", ["git", "clone", "source-super", "super"])
        .success();
    let work_dir = test_env.work_dir("super");
    declare_submodule(&work_dir, "sub", "sub", "../source-sub");

    // "../source-sub" is resolved against the superproject's origin url, not
    // against the working copy.
    let output = work_dir.run_jj(["git", "submodule", "clone"]);
    insta::assert_snapshot!(output, @r#"
    ------- stderr -------
    Cloning submodule "sub" from $TEST_ENV/source-sub
    Cloned submodule "sub": 1 bookmark, 0 tags.
    [EOF]
    "#);
    assert_submodule_repo_at(&submodule_repo_path(&work_dir, "sub"), commit_id);
}

#[test]
fn test_git_submodule_clone_relative_url_without_remote() {
    let test_env = TestEnvironment::default();
    test_env.run_jj_in(".", ["git", "init", "super"]).success();
    let work_dir = test_env.work_dir("super");
    declare_submodule(&work_dir, "sub", "sub", "../source-sub");

    let output = work_dir.run_jj(["git", "submodule", "clone"]);
    insta::assert_snapshot!(output, @r#"
    ------- stderr -------
    Error: Cannot resolve the relative url "../source-sub" of submodule "sub": the superproject has no Git remote to resolve it against
    [EOF]
    [exit status: 1]
    "#);
    assert!(!submodule_repo_path(&work_dir, "sub").exists());
}

#[test]
fn test_git_submodule_clone_nested_submodules() {
    let test_env = TestEnvironment::default();
    let repo = git::init(test_env.env_root().join("source"));
    git::add_commit(
        &repo,
        "refs/heads/main",
        ".gitmodules",
        b"[submodule \"inner\"]\n\tpath = inner\n\turl = https://example.invalid/inner.git\n",
        "message",
        &[],
    );
    git::set_symbolic_reference(&repo, "HEAD", "refs/heads/main");
    test_env.run_jj_in(".", ["git", "init", "super"]).success();
    let work_dir = test_env.work_dir("super");
    declare_submodule(
        &work_dir,
        "sub",
        "sub",
        test_env.env_root().join("source").to_str().unwrap(),
    );

    // Recursion is out of scope, but a submodule whose own submodules are
    // missing looks exactly like one that has none, so it has to be said.
    let output = work_dir.run_jj(["git", "submodule", "clone"]);
    insta::assert_snapshot!(output, @r#"
    ------- stderr -------
    Cloning submodule "sub" from $TEST_ENV/source
    Cloned submodule "sub": 1 bookmark, 0 tags.
    Warning: Submodule "sub" declares submodules of its own, which were not cloned. jj does not clone submodules recursively yet.
    [EOF]
    "#);
}

#[test]
fn test_git_submodule_clone_gitmodules_written_by_git() {
    let test_env = TestEnvironment::default();
    let commit_id = set_up_source(&test_env, "source");
    test_env
        .run_jj_in(".", ["git", "init", "--colocate", "super"])
        .success();
    let work_dir = test_env.work_dir("super");

    // The whole `git submodule add`, so that the `.gitmodules` jj parses is one
    // Git itself wrote rather than one this test made up.
    work_dir
        .run_jj([
            "util",
            "exec",
            "--",
            "git",
            "-c",
            // Git refuses file:// submodules unless asked (CVE-2022-39253).
            "protocol.file.allow=always",
            "submodule",
            "add",
            test_env.env_root().join("source").to_str().unwrap(),
            "sub",
        ])
        .success();

    let output = work_dir.run_jj(["git", "submodule", "clone"]);
    insta::assert_snapshot!(output, @r#"
    ------- stderr -------
    Cloning submodule "sub" from $TEST_ENV/source
    Cloned submodule "sub": 1 bookmark, 0 tags.
    [EOF]
    "#);
    assert_submodule_repo_at(&submodule_repo_path(&work_dir, "sub"), commit_id);
}

#[test]
fn test_git_submodule_is_hidden() {
    let test_env = TestEnvironment::default();
    test_env.run_jj_in(".", ["git", "init", "super"]).success();
    let work_dir = test_env.work_dir("super");

    // The command is incomplete, so it is hidden the way `jj debug` is. Losing
    // this would advertise it as finished.
    let output = work_dir.run_jj(["git", "--help"]);
    assert!(
        !output.stdout.raw().contains("submodule"),
        "`jj git --help` should not list the unfinished submodule commands:\n{}",
        output.stdout.raw()
    );
    // It still runs when named explicitly.
    work_dir
        .run_jj(["git", "submodule", "clone", "--help"])
        .success();
}

#[test]
fn test_git_submodule_empty_repo() {
    let test_env = TestEnvironment::default();
    test_env.run_jj_in(".", ["git", "init", "repo"]).success();
    let work_dir = test_env.work_dir("repo");

    let output = work_dir.run_jj(["git", "submodule", "list"]);
    insta::assert_snapshot!(output, @"");

    let output = work_dir.run_jj(["git", "submodule", "status"]);
    insta::assert_snapshot!(output, @"");
}

#[test]
fn test_git_submodule_no_gitmodules() {
    let test_env = TestEnvironment::default();
    test_env.run_jj_in(".", ["git", "init", "repo"]).success();
    let work_dir = test_env.work_dir("repo");
    work_dir.write_file("file", "contents\n");
    work_dir.create_dir("dir");
    work_dir.write_file("dir/nested", "contents\n");

    let output = work_dir.run_jj(["git", "submodule", "list"]);
    insta::assert_snapshot!(output, @"");

    let output = work_dir.run_jj(["git", "submodule", "status"]);
    insta::assert_snapshot!(output, @"");
}

#[test]
fn test_git_submodule_list() {
    let test_env = TestEnvironment::default();
    let (_source, commit_id) = init_submodule_source(test_env.env_root(), "sub");
    let work_dir = init_superproject(
        &test_env,
        Some(indoc! {r#"
            [submodule "sub"]
            	path = sub
            	url = https://example.org/sub.git
            [submodule "vendored"]
            	path = vendor/thing
            	url = ../thing.git
            	branch = release
            	update = rebase
        "#}),
        &[("sub", &commit_id)],
    );

    let output = work_dir.run_jj(["git", "submodule", "list", "-r", "subs"]);
    insta::assert_snapshot!(output, @"
    sub sub https://example.org/sub.git
    vendored vendor/thing ../thing.git
    [EOF]
    ");

    // A revision that declares nothing reports nothing, even though the store
    // and later revisions may know about these submodules.
    let output = work_dir.run_jj(["git", "submodule", "list", "-r", "subs-"]);
    insta::assert_snapshot!(output, @"");

    let output = work_dir.run_jj([
        "git",
        "submodule",
        "list",
        "-r",
        "subs",
        "-T",
        r#"json(self) ++ "\n""#,
    ]);
    insta::assert_snapshot!(output, @r#"
    {"name":"sub","path":"sub","url":"https://example.org/sub.git","branch":null,"update":null}
    {"name":"vendored","path":"vendor/thing","url":"../thing.git","branch":"release","update":"rebase"}
    [EOF]
    "#);
}

#[test]
fn test_git_submodule_status_not_cloned() {
    let test_env = TestEnvironment::default();
    let (_source, commit_id) = init_submodule_source(test_env.env_root(), "sub");
    let work_dir = init_superproject(
        &test_env,
        Some(indoc! {r#"
            [submodule "sub"]
            	path = sub
            	url = https://example.org/sub.git
        "#}),
        &[("sub", &commit_id)],
    );

    let output = work_dir.run_jj(["git", "submodule", "status", "-r", "subs"]);
    insta::assert_snapshot!(output, @"
    not-cloned         sub              sub de42c77ca733e609bb22cdb5b82dcbc7372c5389
    [EOF]
    ");
}

#[test]
fn test_git_submodule_status_ok_and_not_fetched() {
    let test_env = TestEnvironment::default();
    let env_root = test_env.env_root().to_owned();
    let (fetched_source, fetched_commit) = init_submodule_source(&env_root, "fetched");
    let (stale_source, _stale_first) = init_submodule_source(&env_root, "stale");
    // The clone is taken before the second commit exists, so the store has the
    // submodule but not the commit the superproject points at.
    let stale_clone = clone_submodule_source(&env_root, &stale_source, "stale-clone.git");
    let stale_second = add_submodule_commit(&stale_source);

    let work_dir = init_superproject(
        &test_env,
        Some(indoc! {r#"
            [submodule "fetched"]
            	path = fetched
            	url = https://example.org/fetched.git
            [submodule "stale"]
            	path = stale
            	url = https://example.org/stale.git
        "#}),
        &[("fetched", &fetched_commit), ("stale", &stale_second)],
    );
    add_submodule_repo(
        &test_env,
        &work_dir,
        "fetched",
        &fetched_source.join(".git"),
    );
    add_submodule_repo(&test_env, &work_dir, "stale", &stale_clone);

    let output = work_dir.run_jj(["git", "submodule", "status", "-r", "subs"]);
    insta::assert_snapshot!(output, @"
    ok                 fetched          fetched deb5e1f761335c8ce3ed9f6a7a49075d42a6f449
    not-fetched        stale            stale f648af3fb85673414d5cd98714bd0d531d784387
    [EOF]
    ");

    let output = work_dir.run_jj([
        "git",
        "submodule",
        "status",
        "-r",
        "subs",
        "-T",
        r#"json(self) ++ "\n""#,
    ]);
    insta::assert_snapshot!(output, @r#"
    {"state":"ok","name":"fetched","path":"fetched","url":"https://example.org/fetched.git","commit_id":"deb5e1f761335c8ce3ed9f6a7a49075d42a6f449"}
    {"state":"not-fetched","name":"stale","path":"stale","url":"https://example.org/stale.git","commit_id":"f648af3fb85673414d5cd98714bd0d531d784387"}
    [EOF]
    "#);
}

#[test]
fn test_git_submodule_status_undeclared_repo() {
    let test_env = TestEnvironment::default();
    let env_root = test_env.env_root().to_owned();
    let (declared_source, declared_commit) = init_submodule_source(&env_root, "declared");
    let (gone_source, _) = init_submodule_source(&env_root, "gone");

    let work_dir = init_superproject(
        &test_env,
        Some(indoc! {r#"
            [submodule "declared"]
            	path = declared
            	url = https://example.org/declared.git
        "#}),
        &[("declared", &declared_commit)],
    );
    add_submodule_repo(
        &test_env,
        &work_dir,
        "declared",
        &declared_source.join(".git"),
    );
    // Cloned by some other revision, and this one no longer declares it.
    add_submodule_repo(&test_env, &work_dir, "gone", &gone_source.join(".git"));

    let output = work_dir.run_jj(["git", "submodule", "status", "-r", "subs"]);
    insta::assert_snapshot!(output, @"
    ok                 declared         declared d09015b72f19a0f442037b4cc5a337c14d3c9e4d
    undeclared-repo    gone             (no path)
    [EOF]
    ");
}

#[test]
fn test_git_submodule_status_undeclared_gitlink() {
    let test_env = TestEnvironment::default();
    let env_root = test_env.env_root().to_owned();
    let (declared_source, declared_commit) = init_submodule_source(&env_root, "declared");
    let (_stray_source, stray_commit) = init_submodule_source(&env_root, "stray");

    let work_dir = init_superproject(
        &test_env,
        Some(indoc! {r#"
            [submodule "declared"]
            	path = declared
            	url = https://example.org/declared.git
        "#}),
        &[
            ("declared", &declared_commit),
            ("vendor/stray", &stray_commit),
        ],
    );
    add_submodule_repo(
        &test_env,
        &work_dir,
        "declared",
        &declared_source.join(".git"),
    );

    let output = work_dir.run_jj(["git", "submodule", "status", "-r", "subs"]);
    insta::assert_snapshot!(output, @"
    ok                 declared         declared d09015b72f19a0f442037b4cc5a337c14d3c9e4d
    undeclared-gitlink (no name)        vendor/stray a37053a275a45e4420126619a5947e67238e285b
    [EOF]
    ");
}

#[test]
fn test_git_submodule_status_no_gitlink() {
    let test_env = TestEnvironment::default();
    let env_root = test_env.env_root().to_owned();
    let (source, _) = init_submodule_source(&env_root, "sub");

    // `.gitmodules` still declares the submodule, but this revision's tree has
    // no gitlink at its path.
    let work_dir = init_superproject(
        &test_env,
        Some(indoc! {r#"
            [submodule "sub"]
            	path = sub
            	url = https://example.org/sub.git
        "#}),
        &[],
    );
    add_submodule_repo(&test_env, &work_dir, "sub", &source.join(".git"));

    let output = work_dir.run_jj(["git", "submodule", "status", "-r", "subs"]);
    insta::assert_snapshot!(output, @"
    no-gitlink         sub              sub
    [EOF]
    ");
}

#[test]
fn test_git_submodule_status_no_gitlink_and_not_cloned() {
    let test_env = TestEnvironment::default();
    let work_dir = init_superproject(
        &test_env,
        Some(indoc! {r#"
            [submodule "sub"]
            	path = sub
            	url = https://example.org/sub.git
        "#}),
        &[],
    );

    // Neither cloned nor checked out by this revision: the missing gitlink is
    // the more useful of the two facts, so that is what is reported.
    let output = work_dir.run_jj(["git", "submodule", "status", "-r", "subs"]);
    insta::assert_snapshot!(output, @"
    no-gitlink         sub              sub
    [EOF]
    ");
}
