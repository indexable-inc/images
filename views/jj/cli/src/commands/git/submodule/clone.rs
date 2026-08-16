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

use std::collections::HashSet;
use std::io::Write as _;
use std::path::Path;

use bstr::BStr;
use bstr::BString;
use bstr::ByteSlice as _;
use itertools::Itertools as _;
use jj_lib::git;
use jj_lib::git::GitFetch;
use jj_lib::git::GitFetchRefExpression;
use jj_lib::git::GitImportOptions;
use jj_lib::git::GitSettings;
use jj_lib::git::expand_fetch_refspecs;
use jj_lib::git_backend::GitBackend;
use jj_lib::git_submodule::SubmoduleConfig;
use jj_lib::git_submodule::read_gitmodules;
use jj_lib::ref_name::RemoteName;
use jj_lib::repo::BackendInitializer;
use jj_lib::repo::Repo as _;
use jj_lib::settings::UserSettings;
use jj_lib::signing::Signer;
use jj_lib::store::Store;
use jj_lib::str_util::StringExpression;
use jj_lib::submodule_store::SubmoduleStore;
use jj_lib::submodule_store::init_submodule_repo;
use jj_lib::submodule_store::load_submodule_repo;
use thiserror::Error;

use super::super::ObjectHash;
use crate::cli_util::CommandHelper;
use crate::command_error::CommandError;
use crate::command_error::user_error;
use crate::command_error::user_error_with_message;
use crate::git_util::GitSubprocessUi;
use crate::git_util::load_git_import_options;
use crate::ui::Ui;

/// The remote a submodule's own repo fetches from.
///
/// A `.gitmodules` entry names exactly one url, so the submodule's repo has
/// exactly one remote and there is nothing for the user to choose. Git calls it
/// "origin" too, which keeps `jj git remote list` inside a submodule readable
/// once that is possible.
const SUBMODULE_REMOTE: &RemoteName = RemoteName::new("origin");

/// The superproject remote a relative submodule url is resolved against when
/// the superproject has more than one.
const SUPERPROJECT_DEFAULT_REMOTE: &RemoteName = RemoteName::new("origin");

/// Clone Git submodules into the submodule store
///
/// The submodules are read from `.gitmodules` at the working-copy commit, not
/// from the file on disk, so this works in a workspace where `.gitmodules` was
/// never checked out.
///
/// Each submodule is cloned into `.jj/repo/submodule_store/`, as its own jj
/// repo with its own operation log. Nothing is written to the superproject: no
/// operation is recorded there, and the working copy is left alone. Checking a
/// commit out is what puts a cloned submodule's contents in the working copy.
///
/// A submodule that already has a repo in the store is reported and left alone.
/// Re-running this command after adding a submodule therefore only clones the
/// new one.
#[derive(clap::Args, Clone, Debug)]
pub struct GitSubmoduleCloneArgs {
    /// Names of the submodules to clone (can be repeated)
    ///
    /// These are the `<name>`s of the `[submodule "<name>"]` sections in
    /// `.gitmodules`, which are not always the paths the submodules are checked
    /// out at.
    ///
    /// If no names are given, every submodule declared at the working-copy
    /// commit is cloned.
    #[arg(value_name = "NAME")]
    names: Vec<String>,
}

pub async fn cmd_git_submodule_clone(
    ui: &mut Ui,
    command: &CommandHelper,
    args: &GitSubmoduleCloneArgs,
) -> Result<(), CommandError> {
    let workspace_command = command.workspace_helper(ui).await?;
    let repo = workspace_command.repo().clone();
    let settings = workspace_command.settings();

    let Some(wc_commit_id) = workspace_command.get_wc_commit_id() else {
        return Err(user_error(
            "This command requires a working copy, but this workspace has none",
        ));
    };
    let wc_commit = repo.store().get_commit_async(wc_commit_id).await?;
    let gitmodules = read_gitmodules(&wc_commit.tree())
        .await
        .map_err(|err| {
            user_error_with_message("Failed to read .gitmodules at the working-copy commit", err)
        })?
        .unwrap_or_default();

    // Resolve every name before cloning anything, so that a typo in the last
    // name does not leave the earlier submodules cloned and the command failed.
    let selected: Vec<&SubmoduleConfig> = if args.names.is_empty() {
        gitmodules.iter().collect()
    } else {
        let mut seen = HashSet::new();
        args.names
            .iter()
            .filter(|name| seen.insert(name.as_str()))
            .map(|name| {
                gitmodules.get(name).ok_or_else(|| {
                    user_error(format!(
                        "No submodule named \"{name}\" in .gitmodules at the working-copy commit"
                    ))
                })
            })
            .try_collect()?
    };
    if selected.is_empty() {
        writeln!(
            ui.status(),
            "No submodules are declared in .gitmodules at the working-copy commit."
        )?;
        return Ok(());
    }

    // The submodule's hash algorithm has to match its own remote, which nothing
    // records, so this follows `jj git clone` and takes the configured default
    // rather than the superproject's. A mismatch fails in `git fetch` with
    // Git's own message.
    let object_hash: gix::hash::Kind = settings.get::<ObjectHash>("git.object-hash")?.into();
    let store: &dyn SubmoduleStore = repo.submodule_store().as_ref();

    let mut cloned = 0;
    for config in selected {
        if store.contains(&config.name).map_err(user_error)? {
            // Fetching here instead would make `clone` silently mean "clone or
            // update", and there is no way to ask for the update on its own
            // yet. Leaving the repo alone keeps re-running this command cheap
            // and non-destructive; `jj git submodule fetch` is the command that
            // should update an existing submodule.
            writeln!(
                ui.status(),
                "Skipping submodule \"{name}\": it already has a repo in the submodule store.",
                name = config.name
            )?;
            continue;
        }
        let url = resolve_submodule_url(repo.store(), config)?;
        writeln!(
            ui.status(),
            "Cloning submodule \"{name}\" from {url}",
            name = config.name
        )?;
        clone_submodule(ui, command, settings, store, config, &url, object_hash).await?;
        cloned += 1;
    }
    if cloned == 0 {
        writeln!(ui.status(), "Nothing changed.")?;
    }
    Ok(())
}

/// Clones one submodule into an empty directory of the store.
///
/// The directory is removed again if anything fails, because a half-written
/// repo is worse than no repo at all: [`SubmoduleStore::create`] would refuse
/// to make it a second time and nothing in jj knows how to repair one.
async fn clone_submodule(
    ui: &mut Ui,
    command: &CommandHelper,
    settings: &UserSettings,
    store: &dyn SubmoduleStore,
    config: &SubmoduleConfig,
    url: &str,
    object_hash: gix::hash::Kind,
) -> Result<(), CommandError> {
    let repo_path = store.create(&config.name).map_err(user_error)?;
    let result = clone_into(ui, command, settings, &repo_path, url, object_hash).await;
    if result.is_err()
        && let Err(err) = store.delete(&config.name)
    {
        writeln!(
            ui.warning_default(),
            "Failed to clean up {path} after a failed clone: {err}",
            path = repo_path.display()
        )?;
    }
    let stats = result?;

    writeln!(
        ui.status(),
        "Cloned submodule \"{name}\": {bookmarks}, {tags}.",
        name = config.name,
        bookmarks = count(stats.bookmarks, "bookmark"),
        tags = count(stats.tags, "tag"),
    )?;
    if stats.has_nested_submodules {
        // Recursion is not implemented, and a submodule whose own submodules
        // are missing looks exactly like one that has none, so say it.
        writeln!(
            ui.warning_default(),
            "Submodule \"{name}\" declares submodules of its own, which were not cloned. jj does \
             not clone submodules recursively yet.",
            name = config.name
        )?;
    }
    Ok(())
}

/// What a clone brought in, for reporting.
struct CloneStats {
    bookmarks: usize,
    tags: usize,
    has_nested_submodules: bool,
}

async fn clone_into(
    ui: &mut Ui,
    command: &CommandHelper,
    settings: &UserSettings,
    repo_path: &Path,
    url: &str,
    object_hash: gix::hash::Kind,
) -> Result<CloneStats, CommandError> {
    let backend_initializer: &BackendInitializer = &|settings, store_path| {
        Ok(Box::new(GitBackend::init_internal(
            settings,
            store_path,
            object_hash,
        )?))
    };
    let signer = Signer::from_settings(settings).map_err(user_error)?;
    let repo = init_submodule_repo(settings, repo_path, backend_initializer, signer)
        .await
        .map_err(|err| {
            user_error_with_message(
                format!("Failed to create a repo in {}", repo_path.display()),
                err,
            )
        })?;

    // The remote has to exist in the submodule's Git config before `git fetch`
    // can name it. jj records remotes through a transaction, so the submodule's
    // operation log says where its refs came from.
    let mut tx = repo.start_transaction();
    git::add_remote(tx.repo_mut(), SUBMODULE_REMOTE, url, None)?;
    tx.commit(format!("add git remote {}", SUBMODULE_REMOTE.as_symbol()))
        .await?;

    // Reload so that the `gix::ThreadSafeRepository` behind the store picks up
    // the remote that was just written to the Git config. `jj git clone` reloads
    // the whole workspace at this point for the same reason.
    let repo = load_submodule_repo(settings, repo_path, command.store_factories())
        .await
        .map_err(|err| {
            user_error_with_message(
                format!("Failed to load the repo in {}", repo_path.display()),
                err,
            )
        })?;

    let git_settings = GitSettings::from_settings(settings)?;
    let import_options = GitImportOptions {
        // A fresh clone brings in every commit the remote has. Recording
        // synthetic predecessors for all of them is pure overhead, which is why
        // `jj git clone` turns it off too.
        record_synthetic_predecessors: false,
        ..load_git_import_options(ui, &git_settings, &settings.remote_settings()?)?
    };
    let mut tx = repo.start_transaction();
    let (default_branch, import_stats) = {
        let mut git_fetch = GitFetch::new(
            tx.repo_mut(),
            git_settings.to_subprocess_options(),
            &import_options,
        )?;
        // There is no way to narrow this yet: nothing records which of a
        // submodule's branches a superproject cares about, and the gitlink
        // commit may be reachable from any of them.
        let ref_expr = GitFetchRefExpression {
            bookmark: StringExpression::all(),
            tag: StringExpression::all(),
        };
        let refspecs = expand_fetch_refspecs(SUBMODULE_REMOTE, ref_expr)?;
        git_fetch.fetch(
            SUBMODULE_REMOTE,
            refspecs,
            &mut GitSubprocessUi::new(ui),
            None,
        )?;
        let import_stats = git_fetch.import_refs().await?;
        let default_branch = git_fetch.get_default_branch(SUBMODULE_REMOTE)?;
        (default_branch, import_stats)
    };
    let repo = tx.commit("fetch from git remote into empty repo").await?;

    if !import_stats.failed_ref_names.is_empty() {
        writeln!(ui.warning_default(), "Failed to import some Git refs:")?;
        for name in &import_stats.failed_ref_names {
            writeln!(ui.warning_default(), "  {name}")?;
        }
    }

    // The submodule has no working copy and no checked-out commit, so the
    // default branch is the only commit worth probing for nested submodules.
    let has_nested_submodules = match &default_branch {
        Some(branch) => {
            let symbol = branch.to_remote_symbol(SUBMODULE_REMOTE);
            let target = repo.view().get_remote_bookmark(symbol).target.clone();
            match target.as_normal() {
                Some(commit_id) => {
                    let commit = repo.store().get_commit_async(commit_id).await?;
                    read_gitmodules(&commit.tree())
                        .await
                        .is_ok_and(|file| file.is_some_and(|file| !file.is_empty()))
                }
                None => false,
            }
        }
        None => false,
    };

    Ok(CloneStats {
        bookmarks: import_stats.changed_remote_bookmarks.len(),
        tags: import_stats.changed_remote_tags.len(),
        has_nested_submodules,
    })
}

fn count(n: usize, noun: &str) -> String {
    if n == 1 {
        format!("{n} {noun}")
    } else {
        format!("{n} {noun}s")
    }
}

/// Turns `submodule.<name>.url` into something `git fetch` can be pointed at.
fn resolve_submodule_url(store: &Store, config: &SubmoduleConfig) -> Result<String, CommandError> {
    let Some(relative) = relative_url_path(&config.url) else {
        return url_to_string(&config.url);
    };
    let base = superproject_remote_url(store, config, relative)?;
    let resolved = join_relative_url(&base, relative).map_err(|err| {
        user_error(format!(
            "Cannot resolve the relative url \"{relative}\" of submodule \"{name}\" against the \
             superproject's remote url \"{base}\": {err}",
            name = config.name,
        ))
    })?;
    url_to_string(&resolved)
}

/// The relative path in `url`, if it is a relative submodule url.
///
/// Git decides this on the raw string: a submodule url is relative exactly when
/// it starts with `./` or `../` (`resolve_relative_url()` in
/// `builtin/submodule--helper.c`). gix parses such a string as a local path, so
/// the test is on the path of a file-scheme url.
fn relative_url_path(url: &gix::Url) -> Option<&BStr> {
    if url.scheme != gix::url::Scheme::File {
        return None;
    }
    let path = url.path.as_bstr();
    (path.starts_with(b"./") || path.starts_with(b"../")).then_some(path)
}

/// The superproject remote url a relative submodule url is resolved against.
///
/// Git resolves against the remote of the current branch, defaulting to
/// `origin`. jj does not track a branch, so this prefers `origin` and otherwise
/// accepts a single remote rather than guessing between several.
fn superproject_remote_url(
    store: &Store,
    config: &SubmoduleConfig,
    relative: &BStr,
) -> Result<gix::Url, CommandError> {
    let cannot = |reason: &str| {
        user_error(format!(
            "Cannot resolve the relative url \"{relative}\" of submodule \"{name}\": {reason}",
            name = config.name,
        ))
    };
    let git_repo = git::get_git_repo(store)
        .map_err(|_| cannot("the superproject is not backed by a Git repo"))?;
    let mut names = git::get_all_remote_names(store)?;
    let name = if names.iter().any(|name| name == SUPERPROJECT_DEFAULT_REMOTE) {
        SUPERPROJECT_DEFAULT_REMOTE.to_owned()
    } else if names.len() == 1 {
        names.pop().expect("length was checked")
    } else if names.is_empty() {
        return Err(cannot(
            "the superproject has no Git remote to resolve it against",
        ));
    } else {
        return Err(cannot(&format!(
            "the superproject has several remotes ({remotes}) and none of them is named \
             \"{origin}\"",
            remotes = names.iter().map(|name| name.as_symbol()).join(", "),
            origin = SUPERPROJECT_DEFAULT_REMOTE.as_str(),
        )));
    };
    let remote = git_repo
        .try_find_remote(name.as_str())
        .transpose()
        .map_err(|err| user_error_with_message("Failed to read the superproject's remote", err))?
        .ok_or_else(|| cannot("the superproject has no Git remote to resolve it against"))?;
    let url = remote
        .url(gix::remote::Direction::Fetch)
        .ok_or_else(|| cannot("the superproject's remote has no fetch url"))?;
    Ok(url.clone())
}

/// Applies Git's relative-url arithmetic to a base url.
///
/// Each leading `../` drops the last path component of the base, each leading
/// `./` drops nothing, and what remains is appended. Where Git runs out of
/// components it chops into the scheme and warns, producing a url that cannot
/// work; jj refuses instead.
///
/// A `file` base has to be an absolute path, and on Windows that includes a
/// drive-lettered one: the superproject's remote url there is a path like
/// `D:/checkout/super`, which no leading `/` announces as absolute.
fn join_relative_url(base: &gix::Url, relative: &BStr) -> Result<gix::Url, RelativeUrlError> {
    // A local path is the only url whose components `\` separates. Anywhere
    // else it is an ordinary path character, so which bytes divide the base into
    // components depends on the scheme.
    let local = base.scheme == gix::url::Scheme::File;
    let separator = |byte: u8| byte == b'/' || (local && byte == b'\\');
    if local && !is_absolute_local_path(base.path.as_slice()) {
        // Resolving against a relative base would make the answer depend on the
        // current directory, which is not where either repository lives.
        return Err(RelativeUrlError::RelativeBase);
    }
    let base_path = base.path.as_slice();
    let without_trailing_separators = base_path
        .iter()
        .rposition(|&byte| !separator(byte))
        .map_or(0, |index| index + 1);
    let mut path = BString::from(&base_path[..without_trailing_separators]);
    let mut rest = relative;
    loop {
        if let Some(tail) = rest.strip_prefix(b"../".as_slice()) {
            let last = path
                .iter()
                .rposition(|&byte| separator(byte))
                .ok_or(RelativeUrlError::AboveRoot)?;
            path.truncate(last);
            rest = tail.as_bstr();
        } else if let Some(tail) = rest.strip_prefix(b"./".as_slice()) {
            rest = tail.as_bstr();
        } else {
            break;
        }
    }
    // Git rejoins with `/` whatever it split on, and Windows takes a path with
    // mixed separators, so a `\`-separated base comes back mixed.
    path.push(b'/');
    path.extend_from_slice(rest);

    let mut resolved = base.clone();
    resolved.path = path;
    Ok(resolved)
}

/// Whether a local path is absolute, so that resolving against it does not
/// depend on which directory the process happens to be in.
///
/// Git accepts a leading separator, and in its Windows build a `X:` drive
/// prefix as well (`has_dos_drive_prefix()` in `compat/mingw.h`). jj accepts
/// the drive prefix on every platform, because a `.gitmodules` that resolves on
/// Windows has to resolve the same way when the same repository is read on
/// Linux. It is stricter than Git in one place: `X:sub` names a directory
/// relative to wherever drive `X` is currently sitting, so a separator has to
/// follow the prefix for the path to be absolute.
fn is_absolute_local_path(path: &[u8]) -> bool {
    match path {
        [b'/' | b'\\', ..] => true,
        [drive, b':', rest @ ..] if drive.is_ascii_alphabetic() => {
            matches!(rest, [b'/' | b'\\', ..])
        }
        _ => false,
    }
}

#[derive(Debug, Error)]
enum RelativeUrlError {
    #[error("the remote url is itself relative")]
    RelativeBase,
    #[error("it points above the root of the remote url")]
    AboveRoot,
}

fn url_to_string(url: &gix::Url) -> Result<String, CommandError> {
    String::from_utf8(url.to_bstring().into())
        .map_err(|_| user_error(format!("Submodule url is not valid UTF-8: {url}")))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A `file` url shaped the way gix builds one from a path in `.git/config`.
    ///
    /// The path is substituted rather than parsed, because `gix::url::parse`
    /// only reads a leading drive letter as one when it is compiled for
    /// Windows: everywhere else `D:/checkout` parses as an ssh url with the
    /// host `D`. Assigning the field is what lets a Windows-shaped path be
    /// tested on every platform.
    fn local_url(path: &str) -> gix::Url {
        let mut url = gix::url::parse("/placeholder".into()).unwrap();
        url.path = path.into();
        url
    }

    fn join(base: &str, relative: &str) -> Result<String, RelativeUrlError> {
        let resolved = join_relative_url(&local_url(base), relative.into())?;
        Ok(resolved.path.to_string())
    }

    #[test]
    fn test_join_relative_url_unix_path() {
        assert_eq!(
            join("/checkout/super", "../source-sub").unwrap(),
            "/checkout/source-sub"
        );
        assert_eq!(
            join("/checkout/super/", "../source-sub").unwrap(),
            "/checkout/source-sub"
        );
        assert_eq!(
            join("/checkout/super", "./nested/source-sub").unwrap(),
            "/checkout/super/nested/source-sub"
        );
        assert_eq!(
            join("/a/b/super", "../../source-sub").unwrap(),
            "/a/source-sub"
        );
    }

    #[test]
    fn test_join_relative_url_windows_path() {
        // The shape a Windows superproject's remote url has. Neither spelling
        // starts with a separator, and reading that as a relative path is what
        // made `jj git submodule clone` refuse a relative url on Windows.
        assert_eq!(
            join("D:/checkout/super", "../source-sub").unwrap(),
            "D:/checkout/source-sub"
        );
        // `\` divides components too, and the rejoin uses `/` the way Git's does.
        assert_eq!(
            join(r"D:\checkout\super", "../source-sub").unwrap(),
            r"D:\checkout/source-sub"
        );
        // A drive root has one component to give up and no more.
        assert_eq!(join("D:/super", "../source-sub").unwrap(), "D:/source-sub");
    }

    #[test]
    fn test_join_relative_url_refuses_relative_base() {
        assert!(matches!(
            join("checkout/super", "../source-sub"),
            Err(RelativeUrlError::RelativeBase)
        ));
        assert!(matches!(
            join(r"checkout\super", "../source-sub"),
            Err(RelativeUrlError::RelativeBase)
        ));
        // `D:super` is relative to wherever drive `D` is currently sitting.
        assert!(matches!(
            join("D:super", "../source-sub"),
            Err(RelativeUrlError::RelativeBase)
        ));
    }

    #[test]
    fn test_join_relative_url_refuses_above_root() {
        assert!(matches!(
            join("/super", "../../source-sub"),
            Err(RelativeUrlError::AboveRoot)
        ));
        assert!(matches!(
            join("D:/super", "../../source-sub"),
            Err(RelativeUrlError::AboveRoot)
        ));
        assert!(matches!(
            join(r"D:\super", "../../source-sub"),
            Err(RelativeUrlError::AboveRoot)
        ));
    }
}
