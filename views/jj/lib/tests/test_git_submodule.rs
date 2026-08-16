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

use assert_matches::assert_matches;
use indoc::indoc;
use jj_lib::git_submodule::GitmodulesFile;
use jj_lib::git_submodule::GitmodulesParseError;
use jj_lib::git_submodule::InvalidSubmodulePathError;
use jj_lib::git_submodule::read_gitmodules;
use jj_lib::submodule_store::InvalidSubmoduleNameError;
use jj_lib::submodule_store::SubmoduleName;
use pollster::FutureExt as _;
use testutils::TestRepo;
use testutils::create_tree;
use testutils::repo_path;
use testutils::repo_path_buf;

fn parse(source: &str) -> GitmodulesFile {
    GitmodulesFile::parse(source.as_bytes()).unwrap()
}

fn url_of(file: &GitmodulesFile, name: &str) -> String {
    file.get(name).unwrap().url.to_bstring().to_string()
}

#[test]
fn test_parse_multiple_submodules() {
    let file = parse(indoc! {r#"
        [submodule "lib"]
            path = third_party/lib
            url = https://github.com/jj-vcs/jj.git
            branch = main
        [submodule "docs"]
            path = docs/upstream
            url = ../docs.git
            update = rebase
    "#});

    assert_eq!(file.len(), 2);
    // Keyed and ordered by name, not by the order the sections appear in.
    assert_eq!(
        file.iter().map(|c| c.name.as_str()).collect::<Vec<_>>(),
        ["docs", "lib"]
    );

    let lib = file.get("lib").unwrap();
    assert_eq!(lib.path, repo_path_buf("third_party/lib"));
    assert_eq!(url_of(&file, "lib"), "https://github.com/jj-vcs/jj.git");
    assert_eq!(
        lib.branch,
        Some(gix::submodule::config::Branch::Name("main".into()))
    );
    assert_eq!(lib.update, None);

    let docs = file.get("docs").unwrap();
    assert_eq!(docs.path, repo_path_buf("docs/upstream"));
    // A relative url is kept relative. Resolving it needs the superproject's
    // remote, which a `.gitmodules` blob does not carry.
    assert_eq!(url_of(&file, "docs"), "../docs.git");
    assert_eq!(docs.branch, None);
    assert_eq!(docs.update, Some(gix::submodule::config::Update::Rebase));

    assert_eq!(file.get("nonexistent"), None);
}

#[test]
fn test_parse_ignores_unrelated_sections() {
    let file = parse(indoc! {r#"
        [core]
            repositoryFormatVersion = 0
        [submodule]
            active = .
        [submodule "real"]
            path = real
            url = https://example.org/real.git
    "#});

    assert_eq!(
        file.iter().map(|c| c.name.as_str()).collect::<Vec<_>>(),
        ["real"]
    );
}

#[test]
fn test_parse_name_with_slash() {
    // Git allows separators in submodule names; only `.` and `..` components
    // are out. The name is not required to match the path.
    let file = parse(indoc! {r#"
        [submodule "nested/deep"]
            path = elsewhere
            url = https://example.org/deep.git
    "#});

    let config = file.get("nested/deep").unwrap();
    assert_eq!(config.name.as_str(), "nested/deep");
    assert_eq!(config.path, repo_path_buf("elsewhere"));
}

#[test]
fn test_parse_last_value_of_repeated_key_wins() {
    // Git's config reader takes the last value, and jj must agree with it or
    // the two disagree about where a submodule lives.
    let file = parse(indoc! {r#"
        [submodule "dup"]
            path = first
            url = https://example.org/first.git
            path = second
            url = https://example.org/second.git
    "#});

    assert_eq!(file.get("dup").unwrap().path, repo_path_buf("second"));
    assert_eq!(url_of(&file, "dup"), "https://example.org/second.git");
}

#[test]
fn test_parse_missing_path_is_an_error() {
    let source = indoc! {r#"
        [submodule "no-path"]
            url = https://example.org/no-path.git
    "#};

    assert_matches!(
        GitmodulesFile::parse(source.as_bytes()),
        Err(GitmodulesParseError::Path {
            source: InvalidSubmodulePathError::Config(_),
            ..
        })
    );
}

#[test]
fn test_parse_missing_url_is_an_error() {
    let source = indoc! {r#"
        [submodule "no-url"]
            path = no-url
    "#};

    assert_matches!(
        GitmodulesFile::parse(source.as_bytes()),
        Err(GitmodulesParseError::Url { .. })
    );
}

#[test]
fn test_parse_escaping_path_is_an_error() {
    let source = indoc! {r#"
        [submodule "escape"]
            path = ../outside
            url = https://example.org/escape.git
    "#};

    assert_matches!(
        GitmodulesFile::parse(source.as_bytes()),
        Err(GitmodulesParseError::Path { .. })
    );
}

#[test]
fn test_parse_rejects_invalid_name() {
    let source = indoc! {r#"
        [submodule "../escape"]
            path = escape
            url = https://example.org/escape.git
    "#};

    assert_matches!(
        GitmodulesFile::parse(source.as_bytes()),
        Err(GitmodulesParseError::Name {
            source: InvalidSubmoduleNameError::RelativeComponent { .. },
            ..
        })
    );
}

#[test]
fn test_parse_rejects_command_update_strategy() {
    // Git refuses to run a `!command` that came from `.gitmodules`, since the
    // file is attacker-controlled in a clone.
    let source = indoc! {r#"
        [submodule "evil"]
            path = evil
            url = https://example.org/evil.git
            update = !rm -rf /
    "#};

    assert_matches!(
        GitmodulesFile::parse(source.as_bytes()),
        Err(GitmodulesParseError::Update { .. })
    );
}

#[test]
fn test_parse_syntax_error() {
    assert_matches!(
        GitmodulesFile::parse(b"[submodule \"unterminated\"\n"),
        Err(GitmodulesParseError::Syntax(_))
    );
}

#[test]
fn test_valid_names() {
    for name in [
        "lib",
        "nested/deep",
        "..hidden",
        "a..b",
        "third_party/lib.git",
        "name with spaces",
        "\u{1f600}",
    ] {
        assert!(
            SubmoduleName::new(name).is_ok(),
            "expected {name:?} to be accepted"
        );
    }
}

#[track_caller]
fn rejected(name: &str) -> InvalidSubmoduleNameError {
    SubmoduleName::new(name).expect_err(&format!("expected {name:?} to be rejected"))
}

#[test]
fn test_invalid_names() {
    assert_matches!(rejected(""), InvalidSubmoduleNameError::Empty);
    assert_matches!(
        rejected(".."),
        InvalidSubmoduleNameError::RelativeComponent { .. }
    );
    assert_matches!(
        rejected("."),
        InvalidSubmoduleNameError::RelativeComponent { .. }
    );
    assert_matches!(
        rejected("a/../b"),
        InvalidSubmoduleNameError::RelativeComponent { .. }
    );
    // Both separators count on every platform, so a name rejected on Linux is
    // rejected on Windows too.
    assert_matches!(
        rejected("a\\..\\b"),
        InvalidSubmoduleNameError::RelativeComponent { .. }
    );
    assert_matches!(
        rejected("/absolute"),
        InvalidSubmoduleNameError::LeadingSeparator
    );
    assert_matches!(
        rejected("\\absolute"),
        InvalidSubmoduleNameError::LeadingSeparator
    );
    assert_matches!(
        rejected("bell\u{7}"),
        InvalidSubmoduleNameError::ControlCharacter { .. }
    );
    assert_matches!(
        rejected("new\nline"),
        InvalidSubmoduleNameError::ControlCharacter { .. }
    );
    assert_matches!(
        rejected("c:drive"),
        InvalidSubmoduleNameError::ReservedCharacter { .. }
    );
    assert_matches!(
        rejected("star*"),
        InvalidSubmoduleNameError::ReservedCharacter { .. }
    );
    assert_matches!(
        rejected("trailing."),
        InvalidSubmoduleNameError::TrailingPeriodOrSpace { .. }
    );
    assert_matches!(
        rejected("trailing "),
        InvalidSubmoduleNameError::TrailingPeriodOrSpace { .. }
    );
    assert_matches!(
        rejected("nul"),
        InvalidSubmoduleNameError::ReservedDeviceName { .. }
    );
    assert_matches!(
        rejected("COM1.txt"),
        InvalidSubmoduleNameError::ReservedDeviceName { .. }
    );
    assert_matches!(
        rejected("dir/lpt9"),
        InvalidSubmoduleNameError::ReservedDeviceName { .. }
    );
}

#[test]
fn test_name_validation_is_at_least_as_strict_as_git() {
    // `gix::validate::submodule::name()` is gitoxide's port of git's
    // `check_submodule_name()`. jj layers extra rules on top, so anything gix
    // rejects has to be rejected here too, or jj is accepting a name git would
    // refuse. This catches the two implementations drifting apart.
    for name in [
        "",
        "..",
        "../escape",
        "a/../b",
        "a\\..\\b",
        "lib",
        "nested/deep",
        "a..b",
        "..hidden",
    ] {
        let git_accepts = gix::validate::submodule::name(name.into()).is_ok();
        let jj_accepts = SubmoduleName::new(name).is_ok();
        assert!(
            git_accepts || !jj_accepts,
            "jj accepted {name:?} but git rejects it"
        );
    }
}

#[test]
fn test_get_by_path() {
    let file = parse(indoc! {r#"
        [submodule "renamed"]
            path = new/location
            url = https://example.org/renamed.git
        [submodule "other"]
            path = other
            url = https://example.org/other.git
    "#});

    // A gitlink in a tree gives a path; the name and url come from here.
    let found = file.get_by_path(repo_path("new/location")).unwrap();
    assert_eq!(found.name.as_str(), "renamed");
    assert_eq!(url_of(&file, "renamed"), "https://example.org/renamed.git");

    // The name is not a path, and a prefix of a path is not that path.
    assert!(file.get_by_path(repo_path("renamed")).is_none());
    assert!(file.get_by_path(repo_path("new")).is_none());
    assert!(file.get_by_path(repo_path("new/location/deeper")).is_none());
}

#[test]
fn test_read_gitmodules_from_tree() {
    let test_repo = TestRepo::init();
    let repo = &test_repo.repo;

    // The file is read out of the tree, never off disk, so this works in a
    // workspace where `.gitmodules` was never checked out.
    let tree = create_tree(
        repo,
        &[(
            repo_path(".gitmodules"),
            indoc! {r#"
                [submodule "lib"]
                    path = lib
                    url = https://example.org/lib.git
            "#},
        )],
    );

    let file = read_gitmodules(&tree).block_on().unwrap().unwrap();
    assert_eq!(file.len(), 1);
    assert_eq!(file.get("lib").unwrap().path, repo_path_buf("lib"));
}

#[test]
fn test_read_gitmodules_absent_from_tree() {
    let test_repo = TestRepo::init();
    let repo = &test_repo.repo;

    let tree = create_tree(repo, &[(repo_path("unrelated"), "content")]);

    assert!(read_gitmodules(&tree).block_on().unwrap().is_none());
}

#[test]
fn test_read_gitmodules_ignores_nested_file() {
    let test_repo = TestRepo::init();
    let repo = &test_repo.repo;

    // Only the root `.gitmodules` describes this repository's submodules; one
    // belonging to a submodule of a submodule is not ours to read.
    let tree = create_tree(
        repo,
        &[(
            repo_path("sub/.gitmodules"),
            indoc! {r#"
                [submodule "inner"]
                    path = inner
                    url = https://example.org/inner.git
            "#},
        )],
    );

    assert!(read_gitmodules(&tree).block_on().unwrap().is_none());
}

#[test]
fn test_read_gitmodules_parse_error_from_tree() {
    let test_repo = TestRepo::init();
    let repo = &test_repo.repo;

    let tree = create_tree(
        repo,
        &[(
            repo_path(".gitmodules"),
            indoc! {r#"
                [submodule "no-url"]
                    path = no-url
            "#},
        )],
    );

    assert_matches!(
        read_gitmodules(&tree).block_on(),
        Err(jj_lib::git_submodule::ReadGitmodulesError::Parse(
            GitmodulesParseError::Url { .. }
        ))
    );
}
