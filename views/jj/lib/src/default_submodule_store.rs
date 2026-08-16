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

//! The default [`SubmoduleStore`], which keeps each submodule's repo in a
//! directory under `.jj/repo/submodule_store/repos/`.

use std::fs;
use std::io;
use std::path::Path;
use std::path::PathBuf;

use crate::file_util::IoResultExt as _;
use crate::file_util::PathError;
use crate::submodule_store::SubmoduleName;
use crate::submodule_store::SubmoduleStore;
use crate::submodule_store::SubmoduleStoreError;
use crate::submodule_store::decode_dir_name;
use crate::submodule_store::encode_dir_name;

/// Subdirectory of the store that the submodule repos live in.
///
/// They are one level down rather than directly in the store directory so that
/// the store can grow files of its own without one of them colliding with a
/// submodule's directory. `type`, which the repo writes at init time to record
/// which backend this is, is already such a file.
const REPOS_DIR: &str = "repos";

/// A [`SubmoduleStore`] that keeps each submodule's repo in a directory on
/// disk.
#[derive(Debug)]
pub struct DefaultSubmoduleStore {
    path: PathBuf,
}

impl DefaultSubmoduleStore {
    /// Loads an existing store rooted at `store_path`.
    pub fn load(store_path: &Path) -> Self {
        Self {
            path: store_path.to_path_buf(),
        }
    }

    /// Creates a store rooted at `store_path`, which must already exist.
    ///
    /// Nothing is written: the `repos` directory appears the first time a
    /// submodule is created. That is also what a store written by a jj from
    /// before `repos` existed looks like, so such a store keeps working and
    /// simply reports no submodules.
    pub fn init(store_path: &Path) -> Self {
        Self {
            path: store_path.to_path_buf(),
        }
    }

    /// Name of this backend, as recorded in `.jj/repo/submodule_store/type`.
    pub fn name() -> &'static str {
        "default"
    }

    fn repos_dir(&self) -> PathBuf {
        self.path.join(REPOS_DIR)
    }
}

impl SubmoduleStore for DefaultSubmoduleStore {
    fn name(&self) -> &str {
        Self::name()
    }

    fn repo_path(&self, name: &SubmoduleName) -> Result<PathBuf, SubmoduleStoreError> {
        Ok(self.repos_dir().join(encode_dir_name(name)?))
    }

    fn list(&self) -> Result<Vec<SubmoduleName>, SubmoduleStoreError> {
        let repos_dir = self.repos_dir();
        let entries = match fs::read_dir(&repos_dir) {
            Ok(entries) => entries,
            // No submodule has been created yet, or the store predates `repos`.
            Err(err) if err.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(err) => {
                return Err(PathError {
                    path: repos_dir,
                    source: err,
                }
                .into());
            }
        };
        let mut names = Vec::new();
        for entry in entries {
            let entry = entry.context(&repos_dir)?;
            let path = entry.path();
            // Anything here that is not a submodule repo is reported rather
            // than skipped: a store that hides part of its contents leaves the
            // caller unable to tell an absent submodule from a corrupt one.
            if !entry.file_type().context(&path)?.is_dir() {
                return Err(SubmoduleStoreError::UnexpectedFile { path });
            }
            let name = decode_dir_name(&entry.file_name())
                .map_err(|source| SubmoduleStoreError::InvalidRepoDir { path, source })?;
            names.push(name);
        }
        names.sort_unstable();
        Ok(names)
    }

    fn contains(&self, name: &SubmoduleName) -> Result<bool, SubmoduleStoreError> {
        let path = self.repo_path(name)?;
        // `Path::is_dir` would read an unreadable directory as an absent one,
        // which is the same silent hiding `list` refuses to do.
        match fs::metadata(&path) {
            Ok(metadata) => Ok(metadata.is_dir()),
            Err(err) if err.kind() == io::ErrorKind::NotFound => Ok(false),
            Err(err) => Err(PathError { path, source: err }.into()),
        }
    }

    fn create(&self, name: &SubmoduleName) -> Result<PathBuf, SubmoduleStoreError> {
        let path = self.repo_path(name)?;
        let repos_dir = self.repos_dir();
        fs::create_dir_all(&repos_dir).context(&repos_dir)?;
        match fs::create_dir(&path) {
            Ok(()) => Ok(path),
            Err(err) if err.kind() == io::ErrorKind::AlreadyExists => {
                Err(SubmoduleStoreError::AlreadyExists { name: name.clone() })
            }
            Err(err) => Err(PathError { path, source: err }.into()),
        }
    }

    fn delete(&self, name: &SubmoduleName) -> Result<(), SubmoduleStoreError> {
        let path = self.repo_path(name)?;
        match fs::remove_dir_all(&path) {
            Ok(()) => Ok(()),
            Err(err) if err.kind() == io::ErrorKind::NotFound => {
                Err(SubmoduleStoreError::NotFound { name: name.clone() })
            }
            Err(err) => Err(PathError { path, source: err }.into()),
        }
    }
}
