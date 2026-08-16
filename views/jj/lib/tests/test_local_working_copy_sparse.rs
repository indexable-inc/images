// Copyright 2022 The Jujutsu Authors
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

use futures::StreamExt as _;
use itertools::Itertools as _;
use jj_lib::local_working_copy::LocalWorkingCopy;
use jj_lib::matchers::EverythingMatcher;
use jj_lib::repo::Repo as _;
use jj_lib::repo_path::RepoPath;
use jj_lib::repo_path::RepoPathBuf;
use jj_lib::working_copy::CheckoutOptions;
use jj_lib::working_copy::CheckoutStats;
use jj_lib::working_copy::WorkingCopy as _;
use jj_lib::workspace::Workspace;
use jj_lib::workspace::default_working_copy_factories;
use pollster::FutureExt as _;
use testutils::TestResult;
use testutils::TestWorkspace;
use testutils::commit_with_tree;
use testutils::create_tree;
use testutils::repo_path;
use testutils::user_settings;

fn to_owned_path_vec(paths: &[&RepoPath]) -> Vec<RepoPathBuf> {
    paths.iter().map(|&path| path.to_owned()).collect()
}

#[test]
fn test_sparse_checkout() -> TestResult {
    let mut test_workspace = TestWorkspace::init();
    let repo = &test_workspace.repo;
    let working_copy_path = test_workspace.workspace.workspace_root().to_owned();

    let root_file1_path = repo_path("file1");
    let root_file2_path = repo_path("file2");
    let dir1_path = repo_path("dir1");
    let dir1_file1_path = repo_path("dir1/file1");
    let dir1_file2_path = repo_path("dir1/file2");
    let dir1_subdir1_path = repo_path("dir1/subdir1");
    let dir1_subdir1_file1_path = repo_path("dir1/subdir1/file1");
    let dir2_path = repo_path("dir2");
    let dir2_file1_path = repo_path("dir2/file1");

    let tree = create_tree(
        repo,
        &[
            (root_file1_path, "contents"),
            (root_file2_path, "contents"),
            (dir1_file1_path, "contents"),
            (dir1_file2_path, "contents"),
            (dir1_subdir1_file1_path, "contents"),
            (dir2_file1_path, "contents"),
        ],
    );
    let commit = commit_with_tree(repo.store(), tree);

    test_workspace
        .workspace
        .check_out(
            repo.op_id().clone(),
            None,
            &commit,
            &CheckoutOptions::default(),
        )
        .block_on()?;
    let ws = &mut test_workspace.workspace;

    // Set sparse patterns to only dir1/
    let mut locked_ws = ws.start_working_copy_mutation().block_on()?;
    let sparse_patterns = to_owned_path_vec(&[dir1_path]);
    let stats = locked_ws
        .locked_wc()
        .set_sparse_patterns(sparse_patterns.clone())
        .block_on()?;
    assert_eq!(
        stats,
        CheckoutStats {
            updated_files: 0,
            added_files: 0,
            removed_files: 3,
            skipped_files: 0,
            ..CheckoutStats::default()
        }
    );
    assert_eq!(locked_ws.locked_wc().sparse_patterns()?, sparse_patterns);
    assert!(
        !root_file1_path
            .to_fs_path_unchecked(&working_copy_path)
            .exists()
    );
    assert!(
        !root_file2_path
            .to_fs_path_unchecked(&working_copy_path)
            .exists()
    );
    assert!(
        dir1_file1_path
            .to_fs_path_unchecked(&working_copy_path)
            .exists()
    );
    assert!(
        dir1_file2_path
            .to_fs_path_unchecked(&working_copy_path)
            .exists()
    );
    assert!(
        dir1_subdir1_file1_path
            .to_fs_path_unchecked(&working_copy_path)
            .exists()
    );
    assert!(
        !dir2_file1_path
            .to_fs_path_unchecked(&working_copy_path)
            .exists()
    );

    // Write the new state to disk
    locked_ws.finish(repo.op_id().clone()).block_on()?;
    let wc: &LocalWorkingCopy = ws.working_copy().downcast_ref().unwrap();
    assert_eq!(
        wc.file_states()?.paths().collect_vec(),
        vec![dir1_file1_path, dir1_file2_path, dir1_subdir1_file1_path]
    );
    assert_eq!(wc.sparse_patterns()?, sparse_patterns);

    // Reload the state to check that it was persisted
    let wc = LocalWorkingCopy::load(
        repo.store().clone(),
        ws.workspace_root().to_path_buf(),
        wc.state_path().to_path_buf(),
        repo.settings(),
    )?;
    assert_eq!(
        wc.file_states()?.paths().collect_vec(),
        vec![dir1_file1_path, dir1_file2_path, dir1_subdir1_file1_path]
    );
    assert_eq!(wc.sparse_patterns()?, sparse_patterns);

    // Set sparse patterns to file2, dir1/subdir1/ and dir2/
    let mut locked_wc = wc.start_mutation().block_on()?;
    let sparse_patterns = to_owned_path_vec(&[root_file1_path, dir1_subdir1_path, dir2_path]);
    let stats = locked_wc
        .set_sparse_patterns(sparse_patterns.clone())
        .block_on()?;
    assert_eq!(
        stats,
        CheckoutStats {
            updated_files: 0,
            added_files: 2,
            removed_files: 2,
            skipped_files: 0,
            ..CheckoutStats::default()
        }
    );
    assert_eq!(locked_wc.sparse_patterns()?, sparse_patterns);
    assert!(
        root_file1_path
            .to_fs_path_unchecked(&working_copy_path)
            .exists()
    );
    assert!(
        !root_file2_path
            .to_fs_path_unchecked(&working_copy_path)
            .exists()
    );
    assert!(
        !dir1_file1_path
            .to_fs_path_unchecked(&working_copy_path)
            .exists()
    );
    assert!(
        !dir1_file2_path
            .to_fs_path_unchecked(&working_copy_path)
            .exists()
    );
    assert!(
        dir1_subdir1_file1_path
            .to_fs_path_unchecked(&working_copy_path)
            .exists()
    );
    assert!(
        dir2_file1_path
            .to_fs_path_unchecked(&working_copy_path)
            .exists()
    );
    let wc = locked_wc.finish(repo.op_id().clone()).block_on()?;
    let wc: &LocalWorkingCopy = wc.downcast_ref().unwrap();
    assert_eq!(
        wc.file_states()?.paths().collect_vec(),
        vec![dir1_subdir1_file1_path, dir2_file1_path, root_file1_path]
    );
    Ok(())
}

/// Test that sparse patterns are respected on commit
/// Narrowing costs the size of the change, not the size of the repository.
///
/// The removal half of a sparse-pattern change used to run a tree diff against
/// the empty tree, and its matcher answered `AllRecursively` for every
/// directory the new patterns did not cover. That read one tree object per
/// directory in the repository -- sequentially -- to rediscover paths the
/// working copy could already name from its own file states. Deleting a file
/// needs no tree object, so a correct narrowing reads none at all.
///
/// Asserted as a count rather than a duration: a timing here would be a flaky
/// restatement of the same claim, and the claim is exact.
#[test]
fn test_sparse_narrowing_reads_no_trees() -> TestResult {
    let mut test_workspace = TestWorkspace::init();
    let repo = &test_workspace.repo;
    let working_copy_path = test_workspace.workspace.workspace_root().to_owned();

    let kept_path = repo_path("kept");
    let kept_file_path = repo_path("kept/file");
    // Enough directories elsewhere that a walk of the whole tree cannot be
    // mistaken for a walk of the change.
    let dropped: Vec<RepoPathBuf> = (0..64)
        .map(|i| RepoPathBuf::from_internal_string(format!("dropped{i}/nested/file")).unwrap())
        .collect();
    let mut contents: Vec<(&RepoPath, &str)> = vec![(kept_file_path, "contents")];
    contents.extend(dropped.iter().map(|path| (path.as_ref(), "contents")));
    let tree = create_tree(repo, &contents);
    let commit = commit_with_tree(repo.store(), tree);

    test_workspace
        .workspace
        .check_out(
            repo.op_id().clone(),
            None,
            &commit,
            &CheckoutOptions::default(),
        )
        .block_on()?;
    // Reload so the store's tree cache is cold, as it is in the process a `jj
    // sparse set` actually runs in. A warm cache would answer the whole walk
    // from memory and hide the reads this test is about.
    let mut ws = Workspace::load(
        &user_settings(),
        &working_copy_path,
        &test_workspace.env.default_store_factories(),
        &default_working_copy_factories(),
    )?;

    let reads_before = test_workspace.env.tree_reads();
    let mut locked_ws = ws.start_working_copy_mutation().block_on()?;
    let sparse_patterns = to_owned_path_vec(&[kept_path]);
    let stats = locked_ws
        .locked_wc()
        .set_sparse_patterns(sparse_patterns.clone())
        .block_on()?;
    locked_ws.finish(repo.op_id().clone()).block_on()?;
    let reads = test_workspace.env.tree_reads() - reads_before;

    assert_eq!(
        stats,
        CheckoutStats {
            updated_files: 0,
            added_files: 0,
            removed_files: 64,
            skipped_files: 0,
            ..CheckoutStats::default()
        }
    );
    // One read: the working-copy tree itself, which the mutation resolves before
    // any pattern work. What must not happen is a read per directory being
    // dropped -- 129 of them here, one for each `droppedN` and its `nested`.
    assert!(
        reads <= 1,
        "narrowing read {reads} tree objects for {} removed directories; it should read at most \
         the working-copy tree",
        dropped.len()
    );

    // The files really are gone, and the directories they were alone in with
    // them.
    for path in &dropped {
        assert!(
            !path.to_fs_path_unchecked(&working_copy_path).exists(),
            "{path:?} was not removed"
        );
    }
    assert!(
        !working_copy_path.join("dropped0").exists(),
        "an emptied directory was left behind"
    );
    assert!(
        kept_file_path
            .to_fs_path_unchecked(&working_copy_path)
            .exists()
    );
    Ok(())
}

#[test]
fn test_sparse_commit() -> TestResult {
    let mut test_workspace = TestWorkspace::init();
    let repo = &test_workspace.repo;
    let op_id = repo.op_id().clone();
    let working_copy_path = test_workspace.workspace.workspace_root().to_owned();

    let root_file1_path = repo_path("file1");
    let dir1_path = repo_path("dir1");
    let dir1_file1_path = repo_path("dir1/file1");
    let dir2_path = repo_path("dir2");
    let dir2_file1_path = repo_path("dir2/file1");

    let tree = create_tree(
        repo,
        &[
            (root_file1_path, "contents"),
            (dir1_file1_path, "contents"),
            (dir2_file1_path, "contents"),
        ],
    );

    let commit = commit_with_tree(repo.store(), tree.clone());
    test_workspace
        .workspace
        .check_out(
            repo.op_id().clone(),
            None,
            &commit,
            &CheckoutOptions::default(),
        )
        .block_on()?;

    // Set sparse patterns to only dir1/
    let mut locked_ws = test_workspace
        .workspace
        .start_working_copy_mutation()
        .block_on()?;
    let sparse_patterns = to_owned_path_vec(&[dir1_path]);
    locked_ws
        .locked_wc()
        .set_sparse_patterns(sparse_patterns)
        .block_on()?;
    locked_ws.finish(repo.op_id().clone()).block_on()?;

    // Write modified version of all files, including files that are not in the
    // sparse patterns.
    std::fs::write(
        root_file1_path.to_fs_path_unchecked(&working_copy_path),
        "modified",
    )?;
    std::fs::write(
        dir1_file1_path.to_fs_path_unchecked(&working_copy_path),
        "modified",
    )?;
    std::fs::create_dir(dir2_path.to_fs_path_unchecked(&working_copy_path))?;
    std::fs::write(
        dir2_file1_path.to_fs_path_unchecked(&working_copy_path),
        "modified",
    )?;

    // Create a tree from the working copy. Only dir1/file1 should be updated in the
    // tree.
    let modified_tree = test_workspace.snapshot()?;
    let diff: Vec<_> = tree
        .diff_stream(&modified_tree, &EverythingMatcher)
        .collect()
        .block_on();
    assert_eq!(diff.len(), 1);
    assert_eq!(diff[0].path.as_ref(), dir1_file1_path);

    // Set sparse patterns to also include dir2/
    let mut locked_ws = test_workspace
        .workspace
        .start_working_copy_mutation()
        .block_on()?;
    let sparse_patterns = to_owned_path_vec(&[dir1_path, dir2_path]);
    locked_ws
        .locked_wc()
        .set_sparse_patterns(sparse_patterns)
        .block_on()?;
    locked_ws.finish(op_id).block_on()?;

    // Create a tree from the working copy. Only dir1/file1 and dir2/file1 should be
    // updated in the tree.
    let modified_tree = test_workspace.snapshot()?;
    let diff: Vec<_> = tree
        .diff_stream(&modified_tree, &EverythingMatcher)
        .collect()
        .block_on();
    assert_eq!(diff.len(), 2);
    assert_eq!(diff[0].path.as_ref(), dir1_file1_path);
    assert_eq!(diff[1].path.as_ref(), dir2_file1_path);
    Ok(())
}

#[test]
fn test_sparse_commit_gitignore() -> TestResult {
    // Test that (untracked) .gitignore files in parent directories are respected
    let mut test_workspace = TestWorkspace::init();
    let repo = &test_workspace.repo;
    let working_copy_path = test_workspace.workspace.workspace_root().to_owned();

    let dir1_path = repo_path("dir1");
    let dir1_file1_path = repo_path("dir1/file1");
    let dir1_file2_path = repo_path("dir1/file2");

    // Set sparse patterns to only dir1/
    let mut locked_ws = test_workspace
        .workspace
        .start_working_copy_mutation()
        .block_on()?;
    let sparse_patterns = to_owned_path_vec(&[dir1_path]);
    locked_ws
        .locked_wc()
        .set_sparse_patterns(sparse_patterns)
        .block_on()?;
    locked_ws.finish(repo.op_id().clone()).block_on()?;

    // Write dir1/file1 and dir1/file2 and a .gitignore saying to ignore dir1/file1
    std::fs::write(working_copy_path.join(".gitignore"), "dir1/file1")?;
    std::fs::create_dir(dir1_path.to_fs_path_unchecked(&working_copy_path))?;
    std::fs::write(
        dir1_file1_path.to_fs_path_unchecked(&working_copy_path),
        "contents",
    )?;
    std::fs::write(
        dir1_file2_path.to_fs_path_unchecked(&working_copy_path),
        "contents",
    )?;

    // Create a tree from the working copy. Only dir1/file2 should be updated in the
    // tree because dir1/file1 is ignored.
    let modified_tree = test_workspace.snapshot()?;
    let entries = modified_tree.entries().collect_vec();
    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0].0.as_ref(), dir1_file2_path);
    Ok(())
}
