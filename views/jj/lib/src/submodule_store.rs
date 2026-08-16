// Copyright 2023 The Jujutsu Authors
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

//! Storage for the repositories of a repo's Git submodules.
//!
//! `docs/design/git-submodule-storage.md` settled on storing each submodule as
//! a full jj repo with its own operation log, which jj drives as a whole unit
//! rather than reaching into its commit backend, and which is only ever reached
//! through the superproject. A [`SubmoduleStore`] is where those repos live: it
//! hands out one directory per submodule and does nothing else, so the repo
//! inside a submodule is an ordinary jj repo that the ordinary repo machinery
//! reads and writes.
//!
//! That division is why the trait knows nothing about [`UserSettings`] or
//! [`StoreFactories`]: creating a directory and creating a repo in it are
//! separate steps, taken by [`init_submodule_repo`] and [`load_submodule_repo`]
//! below, and an alternative store implementation does not have to know how a
//! repo is built to be a usable store.
//!
//! [`SubmoduleName`], the key the store is addressed by, lives here rather than
//! in `crate::git_submodule` because every repo has a submodule store whether
//! or not jj was built with the `git` feature: `.jj/repo/submodule_store` is
//! created by [`crate::repo::ReadonlyRepo::init`] unconditionally, so a repo
//! created by a build without `git` has to stay loadable by one with it.

use std::borrow::Borrow;
use std::ffi::OsStr;
use std::fmt;
use std::fmt::Debug;
use std::path::Path;
use std::path::PathBuf;
use std::sync::Arc;

use bstr::BStr;
use bstr::ByteSlice as _;
use thiserror::Error;

use crate::file_util::PathError;
use crate::repo::BackendInitializer;
use crate::repo::ReadonlyRepo;
use crate::repo::RepoInitError;
use crate::repo::RepoLoader;
use crate::repo::RepoLoaderError;
use crate::repo::StoreFactories;
use crate::repo::StoreLoadError;
use crate::settings::UserSettings;
use crate::signing::Signer;

/// The name of a Git submodule.
///
/// This is the `<name>` in a `[submodule "<name>"]` section header. It is the
/// key everything else hangs off: Git stores the submodule's repository under
/// `.git/modules/<name>`, and unlike the checkout path the name does not change
/// when a submodule is moved, so it is stable across history.
///
/// A `SubmoduleName` has been validated by [`SubmoduleName::new`], so it is
/// always safe to use as a relative path.
#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct SubmoduleName(String);

impl SubmoduleName {
    /// Validates `name` and wraps it.
    ///
    /// Git validates submodule names in `check_submodule_name()`
    /// (`submodule-config.c`), which rejects an empty name and any `..` path
    /// component, with both `/` and `\` counted as separators on every platform
    /// so that a name is judged the same way everywhere. That check exists
    /// because the name is appended to a directory path (CVE-2018-11235).
    ///
    /// The same rule is ported to Rust as
    /// [`fn@gix::validate::submodule::name`], which is not called here only
    /// because the extra rules below need the components anyway.
    ///
    /// jj applies the same rules, plus the ones Git applies to any path it
    /// writes to the index (`verify_path()` in `read-cache.c`, and
    /// `is_valid_win32_path()` in `compat/mingw.c`): no `.` component, no
    /// leading separator, no control characters, and nothing that is unusable
    /// as a file name on Windows. Git only enforces the last group when
    /// building for Windows; jj enforces it everywhere, because a repository
    /// that resolves on one platform and not another is worse than a repository
    /// jj refuses outright.
    pub fn new(name: impl Into<String>) -> Result<Self, InvalidSubmoduleNameError> {
        let name = name.into();
        validate_name(&name)?;
        Ok(Self(name))
    }

    /// Like [`SubmoduleName::new`], but for a raw section name straight out of
    /// the config parser, which need not be UTF-8.
    pub fn from_bytes(name: &BStr) -> Result<Self, InvalidSubmoduleNameError> {
        let name = name
            .to_str()
            .map_err(|_| InvalidSubmoduleNameError::NotUtf8)?;
        Self::new(name)
    }

    /// The name as written in `.gitmodules`.
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// Consumes this and returns the underlying string.
    pub fn into_string(self) -> String {
        self.0
    }
}

impl AsRef<str> for SubmoduleName {
    fn as_ref(&self) -> &str {
        self.as_str()
    }
}

impl Borrow<str> for SubmoduleName {
    fn borrow(&self) -> &str {
        self.as_str()
    }
}

impl fmt::Display for SubmoduleName {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Reason a `[submodule "<name>"]` header does not name a usable submodule.
#[derive(Clone, Debug, Eq, Error, PartialEq)]
pub enum InvalidSubmoduleNameError {
    /// The name is `""`.
    #[error("Name is empty")]
    Empty,
    /// The name is not UTF-8, which jj requires even though Git does not.
    #[error("Name is not valid UTF-8")]
    NotUtf8,
    /// The name starts with `/` or `\`, making it an absolute path.
    #[error("Name starts with a path separator")]
    LeadingSeparator,
    /// The name has a `.` or `..` component.
    #[error(r#"Name has a "{component}" path component"#)]
    RelativeComponent {
        /// The offending component, `.` or `..`.
        component: String,
    },
    /// The name has a character below U+0020.
    #[error("Name contains the control character U+{:04X}", u32::from(*.character))]
    ControlCharacter {
        /// The offending character.
        character: char,
    },
    /// The name has a character Windows does not allow in a file name.
    #[error("Name contains {character:?}, which is not allowed in a file name on Windows")]
    ReservedCharacter {
        /// The offending character.
        character: char,
    },
    /// A component ends in `.` or a space, which Windows silently strips.
    #[error(r#"Name component "{component}" ends with a period or space"#)]
    TrailingPeriodOrSpace {
        /// The offending component.
        component: String,
    },
    /// A component is a reserved DOS device name such as `nul` or `com1`.
    #[error(r#"Name component "{component}" is a reserved device name on Windows"#)]
    ReservedDeviceName {
        /// The offending component.
        component: String,
    },
}

/// Characters `is_valid_win32_path()` rejects outright.
const WINDOWS_RESERVED_CHARS: [char; 7] = ['"', '*', ':', '<', '>', '?', '|'];

/// Device names that Windows resolves no matter which directory they appear in,
/// and no matter what extension follows. `com<N>` and `lpt<N>` are handled
/// separately since they are ranges.
const WINDOWS_DEVICE_NAMES: [&str; 5] = ["aux", "con", "conin$", "conout$", "nul"];

fn validate_name(name: &str) -> Result<(), InvalidSubmoduleNameError> {
    if name.is_empty() {
        return Err(InvalidSubmoduleNameError::Empty);
    }
    if name.starts_with(['/', '\\']) {
        return Err(InvalidSubmoduleNameError::LeadingSeparator);
    }
    // Git's is_xplatform_dir_sep(): treat both separators as separators
    // everywhere, so that a name is accepted or rejected consistently across
    // platforms rather than depending on the host.
    for component in name.split(['/', '\\']) {
        validate_name_component(component)?;
    }
    Ok(())
}

fn validate_name_component(component: &str) -> Result<(), InvalidSubmoduleNameError> {
    if component == "." || component == ".." {
        return Err(InvalidSubmoduleNameError::RelativeComponent {
            component: component.to_owned(),
        });
    }
    for character in component.chars() {
        if character < '\u{20}' {
            return Err(InvalidSubmoduleNameError::ControlCharacter { character });
        }
        if WINDOWS_RESERVED_CHARS.contains(&character) {
            return Err(InvalidSubmoduleNameError::ReservedCharacter { character });
        }
    }
    if component.ends_with(['.', ' ']) {
        return Err(InvalidSubmoduleNameError::TrailingPeriodOrSpace {
            component: component.to_owned(),
        });
    }
    // A device name stays a device name whatever extension is appended, so
    // compare only the part before the first period.
    let stem = component.split('.').next().unwrap_or(component);
    if is_windows_device_name(stem) {
        return Err(InvalidSubmoduleNameError::ReservedDeviceName {
            component: component.to_owned(),
        });
    }
    Ok(())
}

fn is_windows_device_name(stem: &str) -> bool {
    let lowered = stem.to_ascii_lowercase();
    if WINDOWS_DEVICE_NAMES.contains(&lowered.as_str()) {
        return true;
    }
    let numbered = |prefix: &str| {
        lowered.strip_prefix(prefix).is_some_and(|digit| {
            matches!(digit, "1" | "2" | "3" | "4" | "5" | "6" | "7" | "8" | "9")
        })
    };
    numbered("com") || numbered("lpt")
}

/// Where the repos of a repo's Git submodules are stored.
///
/// The store is keyed by [`SubmoduleName`] rather than by the path the
/// submodule is checked out at, because the name is what `.gitmodules` keys its
/// sections on and it survives a submodule being moved, whereas the path does
/// not.
///
/// Every method that names a submodule may fail on the name alone, before
/// touching the filesystem, since not every valid submodule name fits in a
/// directory name. See [`encode_dir_name`].
pub trait SubmoduleStore: Send + Sync + Debug {
    /// Name of this backend, as recorded in `.jj/repo/submodule_store/type`.
    fn name(&self) -> &str;

    /// The directory `name`'s repo lives in, whether or not it exists yet.
    ///
    /// This is a pure function of the name, so a caller that has just created a
    /// submodule and a caller that is looking one up agree without consulting
    /// any index.
    fn repo_path(&self, name: &SubmoduleName) -> Result<PathBuf, SubmoduleStoreError>;

    /// The submodules that have a repo in this store, sorted by name.
    ///
    /// This reports the store's actual contents, which need not match the
    /// `.gitmodules` of any particular commit: a submodule deleted in the
    /// working-copy commit still has a repo here, and one added by a commit
    /// that has not been checked out yet does not.
    fn list(&self) -> Result<Vec<SubmoduleName>, SubmoduleStoreError>;

    /// Whether `name` has a repo in this store.
    fn contains(&self, name: &SubmoduleName) -> Result<bool, SubmoduleStoreError>;

    /// Creates an empty directory for `name`'s repo and returns it.
    ///
    /// Fails with [`SubmoduleStoreError::AlreadyExists`] if the submodule
    /// already has one, so that a caller cannot quietly adopt the leftovers of
    /// a submodule it thought it was creating from scratch. The directory is
    /// empty on return; put a repo in it with [`init_submodule_repo`].
    fn create(&self, name: &SubmoduleName) -> Result<PathBuf, SubmoduleStoreError>;

    /// Removes `name`'s repo and everything in it.
    ///
    /// Fails with [`SubmoduleStoreError::NotFound`] if there is nothing to
    /// remove, since a caller asking to delete a submodule that is not there
    /// has a stale idea of what the store holds.
    fn delete(&self, name: &SubmoduleName) -> Result<(), SubmoduleStoreError>;
}

/// Reason a [`SubmoduleStore`] operation failed.
#[derive(Debug, Error)]
pub enum SubmoduleStoreError {
    /// The name does not fit in a directory name. See [`encode_dir_name`].
    #[error(
        "Submodule name is too long: '{name}' encodes to {encoded_len} bytes, but a submodule \
         directory name is limited to {limit} bytes"
    )]
    NameTooLong {
        /// The rejected name.
        name: SubmoduleName,
        /// How long its encoded form would have been.
        encoded_len: usize,
        /// The largest encoded length the store accepts.
        limit: usize,
    },
    /// [`SubmoduleStore::create`] was asked for a submodule that already has a
    /// repo.
    #[error("Submodule '{name}' already exists in the submodule store")]
    AlreadyExists {
        /// The submodule that already had a repo.
        name: SubmoduleName,
    },
    /// [`SubmoduleStore::delete`] was asked for a submodule with no repo.
    #[error("Submodule '{name}' does not exist in the submodule store")]
    NotFound {
        /// The submodule that had no repo.
        name: SubmoduleName,
    },
    /// A directory in the store does not name a submodule, so the store cannot
    /// say what is in it.
    #[error("Unrecognized submodule repository directory {path}")]
    InvalidRepoDir {
        /// The offending directory.
        path: PathBuf,
        /// Why its name could not be turned back into a [`SubmoduleName`].
        #[source]
        source: InvalidSubmoduleDirNameError,
    },
    /// Something that is not a submodule repo is sitting where they live.
    #[error("Unexpected file {path} in the submodule store")]
    UnexpectedFile {
        /// The offending path.
        path: PathBuf,
    },
    /// The store could not be read or written.
    #[error(transparent)]
    Path(#[from] PathError),
}

/// The longest encoded directory name a submodule repo may be stored under.
///
/// Filesystems commonly cap a single path component at 255 bytes (ext4, APFS,
/// NTFS), and a name that overruns that produces a confusing errno from
/// whichever syscall hits it first. 250 keeps a little room for a suffix on a
/// temporary directory next to the real one.
pub const MAX_ENCODED_NAME_LEN: usize = 250;

/// Encodes a submodule name as the name of the directory holding its repo.
///
/// The encoding is the lowercase hex of the name's UTF-8 bytes. It is not for
/// human eyes; it is chosen because a submodule name may legally contain a
/// slash, so using the name directly as a relative path both nests (a submodule
/// named `a/b` would sit inside a directory that a submodule named `a` also
/// wants) and collides. jj's 2023 submodule prototype (jj PR #2015) sanitized
/// names by rewriting `/` as `__`, which is lossy: `a/b` and `a__b` land in one
/// directory, and neither can be turned back into the name a gitlink has to be
/// matched against. Hex is flat, exact and reversible, so [`decode_dir_name`]
/// recovers the name from the directory and the store needs no side index
/// mapping one to the other.
///
/// Only lowercase is produced and only lowercase is accepted back, so a
/// submodule has exactly one directory name rather than a family of them.
pub fn encode_dir_name(name: &SubmoduleName) -> Result<String, SubmoduleStoreError> {
    let bytes = name.as_str().as_bytes();
    let encoded_len = 2 * bytes.len();
    if encoded_len > MAX_ENCODED_NAME_LEN {
        // Truncating would make two names collide, so refuse instead, and say
        // what the limit is: the user can rename the submodule in .gitmodules.
        return Err(SubmoduleStoreError::NameTooLong {
            name: name.clone(),
            encoded_len,
            limit: MAX_ENCODED_NAME_LEN,
        });
    }
    let mut encoded = String::with_capacity(encoded_len);
    for byte in bytes {
        encoded.push(char::from_digit(u32::from(byte >> 4), 16).expect("nibble is a hex digit"));
        encoded.push(char::from_digit(u32::from(byte & 0xf), 16).expect("nibble is a hex digit"));
    }
    Ok(encoded)
}

/// Recovers the submodule name a directory name was made from.
///
/// This is the exact inverse of [`encode_dir_name`], and is how the store
/// answers what it holds without keeping a second copy of the mapping.
pub fn decode_dir_name(dir_name: &OsStr) -> Result<SubmoduleName, InvalidSubmoduleDirNameError> {
    let dir_name = dir_name
        .to_str()
        .ok_or(InvalidSubmoduleDirNameError::NotUtf8)?;
    if dir_name.len() % 2 != 0 {
        return Err(InvalidSubmoduleDirNameError::NotHex);
    }
    let mut bytes = Vec::with_capacity(dir_name.len() / 2);
    for pair in dir_name.as_bytes().chunks_exact(2) {
        let nibble = |byte: u8| match byte {
            b'0'..=b'9' => Some(byte - b'0'),
            // Uppercase is rejected rather than accepted, so that a directory
            // this store did not write is reported instead of shadowing the one
            // it would have written.
            b'a'..=b'f' => Some(byte - b'a' + 10),
            _ => None,
        };
        let high = nibble(pair[0]).ok_or(InvalidSubmoduleDirNameError::NotHex)?;
        let low = nibble(pair[1]).ok_or(InvalidSubmoduleDirNameError::NotHex)?;
        bytes.push(high << 4 | low);
    }
    Ok(SubmoduleName::from_bytes(bytes.as_slice().into())?)
}

/// Reason a directory in a submodule store does not name a submodule.
#[derive(Clone, Debug, Eq, Error, PartialEq)]
pub enum InvalidSubmoduleDirNameError {
    /// The directory name is not UTF-8, so it is not the hex this store writes.
    #[error("Directory name is not valid UTF-8")]
    NotUtf8,
    /// The directory name is not an even number of lowercase hex digits.
    #[error("Directory name is not lowercase hex")]
    NotHex,
    /// The directory name decoded, but not to a name a submodule may have.
    #[error(transparent)]
    Name(#[from] InvalidSubmoduleNameError),
}

/// Creates a jj repo for a submodule in `repo_path`.
///
/// `repo_path` must already exist and be empty, which is what
/// [`SubmoduleStore::create`] returns.
///
/// A submodule gets a repo and not a workspace: its files are checked out
/// inside the superproject's working copy, which the superproject's working
/// copy already owns, so there is no second working copy to initialize and
/// `Workspace::init_*` is the wrong entry point.
pub async fn init_submodule_repo(
    settings: &UserSettings,
    repo_path: &Path,
    backend_initializer: &BackendInitializer<'_>,
    signer: Signer,
) -> Result<Arc<ReadonlyRepo>, RepoInitError> {
    // The submodule's own submodule store is initialized too, since a submodule
    // may itself have submodules, and the design expects that nesting to work
    // by being the same relationship one level down.
    ReadonlyRepo::init(
        settings,
        repo_path,
        backend_initializer,
        signer,
        ReadonlyRepo::default_op_store_initializer(),
        ReadonlyRepo::default_op_heads_store_initializer(),
        ReadonlyRepo::default_index_store_initializer(),
        ReadonlyRepo::default_submodule_store_initializer(),
    )
    .await
}

/// Loads the repo a submodule's directory holds, at its current head
/// operation.
///
/// The submodule's operation log is its own, so this resolves the submodule's
/// head and not the superproject's. The design leaves the two logs unrelated on
/// purpose: a superproject operation drives whatever submodule operations it
/// needs rather than being restored alongside them.
pub async fn load_submodule_repo(
    settings: &UserSettings,
    repo_path: &Path,
    store_factories: &StoreFactories,
) -> Result<Arc<ReadonlyRepo>, LoadSubmoduleRepoError> {
    let loader = RepoLoader::init_from_file_system(settings, repo_path, store_factories)?;
    Ok(loader.load_at_head().await?)
}

/// Reason the repo in a submodule's directory could not be loaded.
#[derive(Debug, Error)]
pub enum LoadSubmoduleRepoError {
    /// The submodule's backends could not be loaded.
    #[error(transparent)]
    StoreLoad(#[from] StoreLoadError),
    /// The submodule's backends loaded, but its head operation did not.
    #[error(transparent)]
    RepoLoad(#[from] RepoLoaderError),
}
