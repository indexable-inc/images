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

use std::ffi::OsStr;
use std::ffi::OsString;
use std::fs;
use std::path::Path;

use assert_matches::assert_matches;
use jj_lib::default_submodule_store::DefaultSubmoduleStore;
use jj_lib::git_backend::GitBackend;
use jj_lib::repo::Repo as _;
use jj_lib::repo::StoreFactories;
use jj_lib::signing::Signer;
use jj_lib::submodule_store::InvalidSubmoduleDirNameError;
use jj_lib::submodule_store::InvalidSubmoduleNameError;
use jj_lib::submodule_store::MAX_ENCODED_NAME_LEN;
use jj_lib::submodule_store::SubmoduleName;
use jj_lib::submodule_store::SubmoduleStore as _;
use jj_lib::submodule_store::SubmoduleStoreError;
use jj_lib::submodule_store::decode_dir_name;
use jj_lib::submodule_store::encode_dir_name;
use jj_lib::submodule_store::init_submodule_repo;
use jj_lib::submodule_store::load_submodule_repo;
use pollster::FutureExt as _;
use tempfile::TempDir;
use testutils::TestRepo;
use testutils::TestRepoBackend;
use testutils::new_temp_dir;
use testutils::user_settings;

fn new_store() -> (TempDir, DefaultSubmoduleStore) {
    let temp_dir = new_temp_dir();
    let store = DefaultSubmoduleStore::init(temp_dir.path());
    (temp_dir, store)
}

fn name(value: &str) -> SubmoduleName {
    SubmoduleName::new(value).unwrap()
}

/// The directory the default store keeps submodule repos in.
fn repos_dir(store_dir: &Path) -> std::path::PathBuf {
    store_dir.join("repos")
}

fn dir_names(store_dir: &Path) -> Vec<OsString> {
    let mut names: Vec<_> = fs::read_dir(repos_dir(store_dir))
        .unwrap()
        .map(|entry| entry.unwrap().file_name())
        .collect();
    names.sort();
    names
}

#[test]
fn test_create_contains_list_delete() {
    let (temp_dir, store) = new_store();
    let lib = name("lib");
    let docs = name("docs");

    assert_eq!(store.list().unwrap(), []);
    assert!(!store.contains(&lib).unwrap());

    let lib_path = store.create(&lib).unwrap();
    assert!(lib_path.is_dir());
    assert_eq!(lib_path, store.repo_path(&lib).unwrap());
    assert!(lib_path.starts_with(repos_dir(temp_dir.path())));
    assert!(store.contains(&lib).unwrap());
    assert!(!store.contains(&docs).unwrap());
    assert_eq!(store.list().unwrap(), vec![lib.clone()]);

    store.create(&docs).unwrap();
    // Sorted by name, not by creation order or by directory name.
    assert_eq!(store.list().unwrap(), [docs.clone(), lib.clone()]);

    // Deleting takes the repo with it, not just an empty directory.
    fs::write(lib_path.join("store"), "contents").unwrap();
    store.delete(&lib).unwrap();
    assert!(!lib_path.exists());
    assert!(!store.contains(&lib).unwrap());
    assert_eq!(store.list().unwrap(), vec![docs]);
}

#[test]
fn test_create_is_exclusive() {
    let (_temp_dir, store) = new_store();
    let lib = name("lib");
    store.create(&lib).unwrap();

    // Adopting the leftovers of an earlier submodule with the same name would
    // hand the caller a repo it did not create and does not know the state of.
    assert_matches!(
        store.create(&lib),
        Err(SubmoduleStoreError::AlreadyExists { name }) if name == lib
    );
}

#[test]
fn test_delete_missing() {
    let (_temp_dir, store) = new_store();
    let lib = name("lib");
    assert_matches!(
        store.delete(&lib),
        Err(SubmoduleStoreError::NotFound { name }) if name == lib
    );

    // Also when other submodules exist, so this is not just "the store is
    // empty".
    store.create(&name("docs")).unwrap();
    assert_matches!(
        store.delete(&lib),
        Err(SubmoduleStoreError::NotFound { name }) if name == lib
    );
}

#[test]
fn test_name_with_slash_does_not_nest() {
    let (temp_dir, store) = new_store();
    let outer = name("a");
    let inner = name("a/b");

    let outer_path = store.create(&outer).unwrap();
    let inner_path = store.create(&inner).unwrap();

    // Used as a relative path, "a/b" would live inside "a"'s own repo.
    assert!(!inner_path.starts_with(&outer_path));
    assert_eq!(
        inner_path.parent(),
        Some(repos_dir(temp_dir.path()).as_path())
    );
    assert_eq!(store.list().unwrap(), [outer, inner]);
}

#[test]
fn test_names_that_sanitizing_would_have_collided() {
    let (_temp_dir, store) = new_store();
    // jj PR #2015 mapped "/" to "__", which maps these two names onto one
    // directory and cannot say which one a directory came from.
    let slashed = name("a/b");
    let underscored = name("a__b");
    assert_eq!(encode_dir_name(&slashed).unwrap(), "612f62");
    assert_eq!(encode_dir_name(&underscored).unwrap(), "615f5f62");

    let slashed_path = store.create(&slashed).unwrap();
    let underscored_path = store.create(&underscored).unwrap();
    assert_ne!(slashed_path, underscored_path);
    assert_eq!(
        store.list().unwrap(),
        [slashed.clone(), underscored.clone()]
    );

    // And each one is still recoverable, which "__" cannot manage.
    store.delete(&slashed).unwrap();
    assert!(!store.contains(&slashed).unwrap());
    assert!(store.contains(&underscored).unwrap());
    assert_eq!(store.list().unwrap(), [underscored]);
}

#[test]
fn test_dir_name_round_trip_through_the_filesystem() {
    let (temp_dir, store) = new_store();
    // A name with a slash, a space and a multi-byte character, since the point
    // of hex is that none of them need escaping.
    let submodule = name("third_party/クレート lib");

    let path = store.create(&submodule).unwrap();
    let encoded = encode_dir_name(&submodule).unwrap();
    assert_eq!(path.file_name(), Some(OsStr::new(&encoded)));

    let on_disk = dir_names(temp_dir.path());
    assert_eq!(on_disk, [OsString::from(&encoded)]);
    assert_eq!(decode_dir_name(&on_disk[0]).unwrap(), submodule);
    assert_eq!(store.list().unwrap(), [submodule]);
}

#[test]
fn test_dir_name_encoding_is_lowercase_hex() {
    assert_eq!(encode_dir_name(&name("lib")).unwrap(), "6c6962");
    assert_eq!(decode_dir_name(OsStr::new("6c6962")).unwrap(), name("lib"));
    // Only lowercase is written, so only lowercase is a directory this store
    // could have produced.
    assert_matches!(
        decode_dir_name(OsStr::new("6C6962")),
        Err(InvalidSubmoduleDirNameError::NotHex)
    );
    assert_matches!(
        decode_dir_name(OsStr::new("6c696")),
        Err(InvalidSubmoduleDirNameError::NotHex)
    );
    assert_matches!(
        decode_dir_name(OsStr::new("zz")),
        Err(InvalidSubmoduleDirNameError::NotHex)
    );
    // Valid hex that is not a valid submodule name.
    assert_matches!(
        decode_dir_name(OsStr::new("2e2e")),
        Err(InvalidSubmoduleDirNameError::Name(
            InvalidSubmoduleNameError::RelativeComponent { .. }
        ))
    );
    // Valid hex that is not UTF-8.
    assert_matches!(
        decode_dir_name(OsStr::new("ff")),
        Err(InvalidSubmoduleDirNameError::Name(
            InvalidSubmoduleNameError::NotUtf8
        ))
    );
}

#[test]
fn test_name_too_long_is_rejected() {
    let (_temp_dir, store) = new_store();
    let longest = name(&"a".repeat(MAX_ENCODED_NAME_LEN / 2));
    store.create(&longest).unwrap();
    assert_eq!(store.list().unwrap(), [longest]);

    let too_long = name(&"a".repeat(MAX_ENCODED_NAME_LEN / 2 + 1));
    // Truncating would silently merge two submodules, so the name is refused
    // and the limit is named.
    assert_matches!(
        store.repo_path(&too_long),
        Err(SubmoduleStoreError::NameTooLong {
            encoded_len: 252,
            limit: MAX_ENCODED_NAME_LEN,
            ..
        })
    );
    let message = store.create(&too_long).unwrap_err().to_string();
    assert!(message.contains(too_long.as_str()), "{message}");
    assert!(message.contains("252"), "{message}");
    assert!(
        message.contains(&MAX_ENCODED_NAME_LEN.to_string()),
        "{message}"
    );
    assert_matches!(
        store.delete(&too_long),
        Err(SubmoduleStoreError::NameTooLong { .. })
    );
}

#[test]
fn test_list_reports_junk_directory() {
    let (temp_dir, store) = new_store();
    store.create(&name("lib")).unwrap();
    let junk = repos_dir(temp_dir.path()).join("not-hex");
    fs::create_dir(&junk).unwrap();

    // Skipping the junk directory would leave the store quietly reporting less
    // than it holds, which is worse than reporting the corruption.
    let err = store.list().unwrap_err();
    assert_matches!(
        &err,
        SubmoduleStoreError::InvalidRepoDir {
            path,
            source: InvalidSubmoduleDirNameError::NotHex,
        } if path == &junk
    );
    let message = err.to_string();
    assert!(message.contains("not-hex"), "{message}");
}

#[test]
fn test_list_reports_directory_that_is_not_a_name() {
    let (temp_dir, store) = new_store();
    // Valid hex, decodes to "..", which is exactly the traversal that made Git
    // validate submodule names in the first place.
    let junk = repos_dir(temp_dir.path()).join("2e2e");
    fs::create_dir_all(&junk).unwrap();

    assert_matches!(
        store.list().unwrap_err(),
        SubmoduleStoreError::InvalidRepoDir {
            path,
            source: InvalidSubmoduleDirNameError::Name(
                InvalidSubmoduleNameError::RelativeComponent { .. }
            ),
        } if path == junk
    );
}

#[test]
fn test_list_reports_stray_file() {
    let (temp_dir, store) = new_store();
    store.create(&name("lib")).unwrap();
    let stray = repos_dir(temp_dir.path()).join("6c6963");
    fs::write(&stray, "").unwrap();

    assert_matches!(
        store.list().unwrap_err(),
        SubmoduleStoreError::UnexpectedFile { path } if path == stray
    );
}

#[test]
fn test_store_predating_the_repos_directory() {
    let temp_dir = new_temp_dir();
    // What a store written by a jj from before this layout looks like: the
    // `type` file the repo writes at init time, and nothing else.
    fs::write(temp_dir.path().join("type"), "default").unwrap();
    let store = DefaultSubmoduleStore::load(temp_dir.path());

    assert_eq!(store.list().unwrap(), []);
    assert!(!store.contains(&name("lib")).unwrap());
    // And it starts holding submodules without needing a migration.
    store.create(&name("lib")).unwrap();
    assert_eq!(store.list().unwrap(), [name("lib")]);
    // The store's own files are not submodules, because submodules are one
    // level down.
    assert!(temp_dir.path().join("type").is_file());
}

#[test]
fn test_repo_submodule_store_is_the_default_one() {
    let test_repo = TestRepo::init();
    let store = test_repo.repo.submodule_store();
    assert_eq!(store.name(), DefaultSubmoduleStore::name());

    let lib = name("lib");
    let path = store.create(&lib).unwrap();
    // `ReadonlyRepo::init` canonicalizes the repo path, so compare canonical
    // forms rather than the path the test handed it.
    let repo_path = dunce::canonicalize(test_repo.repo_path()).unwrap();
    assert!(path.starts_with(repo_path.join("submodule_store")));
    assert_eq!(store.list().unwrap(), [lib]);
}

#[test]
fn test_init_and_load_submodule_repo() {
    let settings = user_settings();
    let test_repo = TestRepo::init_with_backend(TestRepoBackend::Git);
    let store = test_repo.repo.submodule_store();
    let lib = name("third_party/lib");
    let submodule_dir = store.create(&lib).unwrap();

    // A submodule gets a repo of its own and no workspace: its files live in
    // the superproject's working copy.
    let submodule_repo = init_submodule_repo(
        &settings,
        &submodule_dir,
        &|settings, store_path| {
            Ok(Box::new(GitBackend::init_internal(
                settings,
                store_path,
                gix::hash::Kind::default(),
            )?))
        },
        Signer::from_settings(&settings).unwrap(),
    )
    .block_on()
    .unwrap();
    assert!(!submodule_dir.join("working_copy").exists());
    // Its own operation log, per the storage design.
    assert!(submodule_dir.join("op_store").is_dir());
    // And its own submodule store, so a nested submodule has somewhere to go.
    assert!(submodule_repo.submodule_store().list().unwrap().is_empty());

    let loaded = load_submodule_repo(&settings, &submodule_dir, &StoreFactories::default())
        .block_on()
        .unwrap();
    assert_eq!(loaded.op_id(), submodule_repo.op_id());
    assert_eq!(
        loaded.store().root_commit_id(),
        submodule_repo.store().root_commit_id()
    );

    // The superproject's store is untouched by the submodule's.
    assert_eq!(test_repo.repo.submodule_store().list().unwrap(), [lib]);
}
