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

//! Reading Git's `.gitmodules` file.
//!
//! `.gitmodules` is Git configuration syntax living at the root of a tree, with
//! one `[submodule "<name>"]` section per submodule. The parsing itself is done
//! by `gix-submodule`; this module adapts the result to jj's types, most
//! importantly by turning the recorded checkout path into a [`RepoPathBuf`] so
//! that callers can look it up in a tree.
//!
//! The file is read out of a tree rather than off disk, because a jj workspace
//! need not have `.gitmodules` checked out at all.

use std::collections::BTreeMap;
use std::collections::HashMap;
use std::sync::Arc;

use bstr::BStr;
use bstr::BString;
use bstr::ByteSlice as _;
use futures::StreamExt as _;
use thiserror::Error;

use crate::backend::BackendError;
use crate::backend::CommitId;
use crate::backend::TreeValue;
use crate::matchers::EverythingMatcher;
use crate::merged_tree::MergedTree;
use crate::merged_tree::TreeDiffEntry;
use crate::repo::ReadonlyRepo;
use crate::repo::Repo as _;
use crate::repo::StoreFactories;
use crate::repo_path::InvalidNewRepoPathError;
use crate::repo_path::RepoPath;
use crate::repo_path::RepoPathBuf;
use crate::settings::UserSettings;
use crate::submodule_store::InvalidSubmoduleNameError;
use crate::submodule_store::LoadSubmoduleRepoError;
use crate::submodule_store::SubmoduleName;
use crate::submodule_store::SubmoduleStore;
use crate::submodule_store::SubmoduleStoreError;
use crate::submodule_store::load_submodule_repo;
use crate::working_copy::SubmoduleContents;
use crate::working_copy::SubmoduleSource;

/// Path of the `.gitmodules` file, relative to the repository root.
pub fn gitmodules_path() -> &'static RepoPath {
    RepoPath::from_internal_string(".gitmodules").expect("statically known valid path")
}

/// One submodule's configuration, as recorded in `.gitmodules`.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SubmoduleConfig {
    /// The `<name>` of the `[submodule "<name>"]` section this came from.
    pub name: SubmoduleName,
    /// Where the submodule is checked out, relative to the repository root.
    ///
    /// This is the path a gitlink occupies in a tree, so it is what a tree
    /// walker matches against, but it is not stable: moving a submodule changes
    /// the path and leaves the name alone.
    pub path: RepoPathBuf,
    /// Where the submodule is cloned and fetched from.
    ///
    /// This may be relative (`../sibling.git`), in which case it is resolved
    /// against the superproject's remote rather than the filesystem.
    pub url: gix::Url,
    /// `submodule.<name>.branch`, if set.
    pub branch: Option<gix::submodule::config::Branch>,
    /// `submodule.<name>.update`, if set.
    ///
    /// Note that `!command` is rejected here rather than reported, since Git
    /// does not honor it when it comes from `.gitmodules`.
    pub update: Option<gix::submodule::config::Update>,
}

/// The submodules declared by a `.gitmodules` file.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct GitmodulesFile {
    by_name: BTreeMap<SubmoduleName, SubmoduleConfig>,
}

impl GitmodulesFile {
    /// Parses the contents of a `.gitmodules` blob.
    ///
    /// Every section is validated up front, so a file with one bad entry fails
    /// as a whole instead of quietly dropping that entry. Sections other than
    /// `submodule.<name>.*` are ignored, as are keys jj has no use for.
    pub fn parse(source: &[u8]) -> Result<Self, GitmodulesParseError> {
        // gix takes a second config to layer over the file, which is how Git
        // lets `.git/config` override a `url` the file cannot be trusted for.
        // There is no superproject config here: this function parses a blob and
        // nothing more, so overrides are the caller's business.
        let no_overrides = gix::config::File::new(gix::config::file::Metadata::api());
        let file = gix::submodule::File::from_bytes(source, None, &no_overrides)
            .map_err(|err| GitmodulesParseError::Syntax(Box::new(err)))?;

        let mut by_name = BTreeMap::new();
        for raw_name in file.names() {
            let name = SubmoduleName::from_bytes(raw_name).map_err(|source| {
                GitmodulesParseError::Name {
                    name: raw_name.to_owned(),
                    source,
                }
            })?;
            let path =
                parse_path(&file, raw_name).map_err(|source| GitmodulesParseError::Path {
                    name: name.clone(),
                    source,
                })?;
            let url = file
                .url(raw_name)
                .map_err(|err| GitmodulesParseError::Url {
                    name: name.clone(),
                    source: Box::new(err),
                })?;
            let branch = file
                .branch(raw_name)
                .map_err(|err| GitmodulesParseError::Branch {
                    name: name.clone(),
                    source: Box::new(err),
                })?;
            let update = file
                .update(raw_name)
                .map_err(|err| GitmodulesParseError::Update {
                    name: name.clone(),
                    source: Box::new(err),
                })?;
            let config = SubmoduleConfig {
                name: name.clone(),
                path,
                url,
                branch,
                update,
            };
            by_name.insert(name, config);
        }
        Ok(Self { by_name })
    }

    /// Looks up a submodule by name.
    pub fn get(&self, name: &str) -> Option<&SubmoduleConfig> {
        self.by_name.get(name)
    }

    /// Looks up a submodule by the path it is checked out at.
    ///
    /// A gitlink in a tree gives a path and nothing else, so this is how a tree
    /// walker gets from what it found to the name and url it needs.
    pub fn get_by_path(&self, path: &RepoPath) -> Option<&SubmoduleConfig> {
        // A `.gitmodules` file has a handful of entries in all but pathological
        // repositories, so a scan beats maintaining a second index.
        self.by_name.values().find(|config| &*config.path == path)
    }

    /// All submodules, ordered by name.
    pub fn by_name(&self) -> &BTreeMap<SubmoduleName, SubmoduleConfig> {
        &self.by_name
    }

    /// Iterates over all submodules, ordered by name.
    pub fn iter(&self) -> impl ExactSizeIterator<Item = &SubmoduleConfig> {
        self.by_name.values()
    }

    /// Whether the file declared no submodules.
    pub fn is_empty(&self) -> bool {
        self.by_name.is_empty()
    }

    /// How many submodules the file declared.
    pub fn len(&self) -> usize {
        self.by_name.len()
    }
}

impl<'a> IntoIterator for &'a GitmodulesFile {
    type Item = &'a SubmoduleConfig;
    type IntoIter = std::collections::btree_map::Values<'a, SubmoduleName, SubmoduleConfig>;

    fn into_iter(self) -> Self::IntoIter {
        self.by_name.values()
    }
}

fn parse_path(
    file: &gix::submodule::File,
    name: &BStr,
) -> Result<RepoPathBuf, InvalidSubmodulePathError> {
    // gix rejects a missing, empty, absolute, or worktree-escaping path.
    let path = file.path(name).map_err(Box::new)?;
    let path = path
        .to_str()
        .map_err(|_| InvalidSubmodulePathError::NotUtf8)?;
    // Git writes `/` separators into `.gitmodules` regardless of platform, and
    // this is a repository file, so split it the same way everywhere instead of
    // deferring to the host's path rules.
    for component in path.split('/') {
        if component == "." || component == ".." {
            return Err(InvalidSubmodulePathError::RelativeComponent {
                component: component.to_owned(),
            });
        }
    }
    Ok(RepoPathBuf::from_internal_string(path)?)
}

/// Reason `submodule.<name>.path` cannot be turned into a [`RepoPathBuf`].
#[derive(Debug, Error)]
pub enum InvalidSubmodulePathError {
    /// The path is missing, empty, absolute, or escapes the worktree.
    #[error(transparent)]
    Config(#[from] Box<gix::submodule::config::path::Error>),
    /// The path is not UTF-8, which jj requires even though Git does not.
    #[error("Path is not valid UTF-8")]
    NotUtf8,
    /// The path has a `.` or `..` component, so it does not name a tree entry.
    #[error(r#"Path has a "{component}" component"#)]
    RelativeComponent {
        /// The offending component.
        component: String,
    },
    /// The path is not a valid repository path for some other reason, such as
    /// an empty component.
    #[error(transparent)]
    InvalidRepoPath(#[from] InvalidNewRepoPathError),
}

/// Reason a `.gitmodules` blob could not be parsed.
#[derive(Debug, Error)]
pub enum GitmodulesParseError {
    /// The blob is not valid Git configuration syntax.
    #[error("Failed to parse .gitmodules")]
    Syntax(#[source] Box<gix::config::parse::Error>),
    /// A section header does not name a usable submodule.
    #[error("Invalid name for submodule '{name}'")]
    Name {
        /// The rejected name, as written in the file.
        name: BString,
        /// Which rule the name broke.
        #[source]
        source: InvalidSubmoduleNameError,
    },
    /// `submodule.<name>.path` is missing or unusable.
    #[error("Invalid path for submodule '{name}'")]
    Path {
        /// The submodule whose path was rejected.
        name: SubmoduleName,
        /// Which rule the path broke.
        #[source]
        source: InvalidSubmodulePathError,
    },
    /// `submodule.<name>.url` is missing or unparsable.
    #[error("Invalid url for submodule '{name}'")]
    Url {
        /// The submodule whose url was rejected.
        name: SubmoduleName,
        /// Which rule the url broke.
        #[source]
        source: Box<gix::submodule::config::url::Error>,
    },
    /// `submodule.<name>.branch` is not a valid fetch refspec source.
    #[error("Invalid branch for submodule '{name}'")]
    Branch {
        /// The submodule whose branch was rejected.
        name: SubmoduleName,
        /// Which rule the branch broke.
        #[source]
        source: Box<gix::submodule::config::branch::Error>,
    },
    /// `submodule.<name>.update` is unrecognized, or is a `!command`, which Git
    /// does not honor from `.gitmodules`.
    #[error("Invalid update strategy for submodule '{name}'")]
    Update {
        /// The submodule whose update strategy was rejected.
        name: SubmoduleName,
        /// Which rule the update strategy broke.
        #[source]
        source: Box<gix::submodule::config::update::Error>,
    },
}

/// Reads and parses `.gitmodules` from the root of `tree`.
///
/// Returns `Ok(None)` if the tree has no `.gitmodules`, which is the common
/// case. Reading from the tree rather than from disk is what makes this work in
/// a workspace where the file was never checked out, and is also what lets a
/// caller ask about a revision other than the working copy.
pub async fn read_gitmodules(
    tree: &MergedTree,
) -> Result<Option<GitmodulesFile>, ReadGitmodulesError> {
    let path = gitmodules_path();
    let value = tree.path_value(path).await?;
    let Ok(value) = value.into_resolved() else {
        return Err(ReadGitmodulesError::Conflicted);
    };
    let id = match value {
        None => return Ok(None),
        Some(TreeValue::File { id, .. }) => id,
        Some(_) => return Err(ReadGitmodulesError::NotAFile),
    };
    let mut reader = tree.store().read_file(path, &id).await?;
    let mut source = Vec::new();
    futures::AsyncReadExt::read_to_end(&mut reader, &mut source)
        .await
        .map_err(|err| BackendError::ReadFile {
            path: path.to_owned(),
            id: id.clone(),
            source: err.into(),
        })?;
    Ok(Some(GitmodulesFile::parse(&source)?))
}

/// Resolves the gitlinks that differ between two trees to the contents a
/// checkout should populate their directories with.
///
/// The working copy cannot do this itself: a gitlink names a commit in the
/// submodule's own repo, that repo lives in `submodule_store`, and loading it
/// needs the same settings and store factories the superproject was loaded
/// with. So this runs where those are available and hands the working copy
/// plain data.
///
/// Only the gitlinks that differ between the two trees are resolved, because
/// those are exactly the ones a checkout will act on. A gitlink that is
/// unchanged leaves its directory as it is, populated or not.
///
/// Nothing here is an error just because a submodule is missing. A submodule
/// that has never been cloned, one whose commit has not been fetched, and one
/// that `.gitmodules` does not declare all resolve to a value saying so, which
/// the working copy reports and the user can act on.
pub async fn resolve_submodule_contents(
    old_tree: &MergedTree,
    new_tree: &MergedTree,
    submodule_store: &dyn SubmoduleStore,
    settings: &UserSettings,
    store_factories: &StoreFactories,
) -> Result<SubmoduleContents, ResolveSubmoduleContentsError> {
    // A submodule being added or removed means only one of the two trees
    // declares it, so both files are consulted and the new tree wins where they
    // disagree, matching which tree the checkout is heading for.
    let mut by_path: BTreeMap<RepoPathBuf, SubmoduleName> = BTreeMap::new();
    for tree in [old_tree, new_tree] {
        if let Some(file) = read_gitmodules(tree).await? {
            for config in file.iter() {
                by_path.insert(config.path.clone(), config.name.clone());
            }
        }
    }

    let mut contents = SubmoduleContents::default();
    let mut repos: HashMap<SubmoduleName, Arc<ReadonlyRepo>> = HashMap::new();
    let mut diff = old_tree.diff_stream(new_tree, &EverythingMatcher);
    while let Some(TreeDiffEntry { path, values }) = diff.next().await {
        let diff = values?;
        for value in [diff.before, diff.after] {
            let Ok(Some(TreeValue::GitSubmodule(commit_id))) = value.into_resolved() else {
                continue;
            };
            let source = resolve_one(
                by_path.get(&path),
                &commit_id,
                submodule_store,
                settings,
                store_factories,
                &mut repos,
            )
            .await?;
            contents.insert(path.clone(), commit_id, source);
        }
    }
    Ok(contents)
}

async fn resolve_one(
    name: Option<&SubmoduleName>,
    commit_id: &CommitId,
    submodule_store: &dyn SubmoduleStore,
    settings: &UserSettings,
    store_factories: &StoreFactories,
    repos: &mut HashMap<SubmoduleName, Arc<ReadonlyRepo>>,
) -> Result<SubmoduleSource, ResolveSubmoduleContentsError> {
    // A gitlink `.gitmodules` does not declare has no name, so there is nothing
    // to look up in the store and no url to clone it from either.
    let Some(name) = name else {
        return Ok(SubmoduleSource::NotCloned);
    };
    let repo = match repos.get(name) {
        Some(repo) => repo.clone(),
        None => {
            if !submodule_store.contains(name)? {
                return Ok(SubmoduleSource::NotCloned);
            }
            let repo_path = submodule_store.repo_path(name)?;
            let repo = load_submodule_repo(settings, &repo_path, store_factories)
                .await
                .map_err(|source| ResolveSubmoduleContentsError::LoadRepo {
                    name: name.clone(),
                    source: Box::new(source),
                })?;
            repos.entry(name.clone()).or_insert(repo).clone()
        }
    };
    match repo.store().get_commit_async(commit_id).await {
        Ok(commit) => Ok(SubmoduleSource::Tree(commit.tree())),
        Err(BackendError::ObjectNotFound { .. }) => Ok(SubmoduleSource::CommitNotFetched),
        Err(err) => Err(ResolveSubmoduleContentsError::Backend(err)),
    }
}

/// Reason a tree's gitlinks could not be resolved.
#[derive(Debug, Error)]
pub enum ResolveSubmoduleContentsError {
    /// `.gitmodules` could not be read out of one of the trees.
    #[error(transparent)]
    ReadGitmodules(#[from] ReadGitmodulesError),
    /// The submodule store could not say what it holds.
    #[error(transparent)]
    SubmoduleStore(#[from] SubmoduleStoreError),
    /// A submodule's repo is in the store but could not be loaded.
    #[error("Failed to load the repo of submodule '{name}'")]
    LoadRepo {
        /// The submodule whose repo would not load.
        name: SubmoduleName,
        /// Why it would not load.
        #[source]
        source: Box<LoadSubmoduleRepoError>,
    },
    /// A tree or commit could not be read.
    #[error(transparent)]
    Backend(#[from] BackendError),
}

/// Reason `.gitmodules` could not be read out of a tree.
#[derive(Debug, Error)]
pub enum ReadGitmodulesError {
    /// The tree could not be read.
    #[error(transparent)]
    Backend(#[from] BackendError),
    /// `.gitmodules` is conflicted, so there is no single file to parse.
    #[error(".gitmodules is conflicted")]
    Conflicted,
    /// `.gitmodules` exists but is a directory or symlink.
    #[error(".gitmodules is not a file")]
    NotAFile,
    /// `.gitmodules` was read but could not be parsed.
    #[error(transparent)]
    Parse(#[from] GitmodulesParseError),
}
