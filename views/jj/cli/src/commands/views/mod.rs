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

//! Subtrees of this repository that are published as repositories of their own.
//!
//! A view is a path prefix plus the repository that prefix is published to. The
//! history under the prefix filters to a history whose hashes are exactly the
//! standalone repository's, so publishing a view is an ordinary `git push` of
//! an ordinary ref -- the only part that is not ordinary is knowing which ref,
//! to which URL. That knowledge is what the `views` config table holds.

mod add;
mod anchor;
mod check;
mod fetch;
mod patches;
mod prompt;
mod push;
mod record;
mod status;
mod tree;

use std::collections::BTreeMap;
use std::collections::BTreeSet;
use std::collections::HashSet;
use std::fs;
use std::io::Write as _;
use std::path::PathBuf;
use std::process::Command;
use std::process::Stdio;

use gix::prelude::Write as _;
use itertools::Itertools as _;
use jj_lib::backend::CommitId;
use jj_lib::backend::TreeValue;
use jj_lib::merged_tree::MergedTree;
use jj_lib::object_id::ObjectId as _;
use jj_lib::ref_name::RefNameBuf;
use jj_lib::repo::Repo as _;
use jj_lib::repo_path::RepoPath;
use jj_lib::repo_path::RepoPathBuf;
use jj_views::Cache;
use jj_views::Elide;
use jj_views::Filter;
use jj_views::Semantics;

use crate::cli_util::CommandHelper;
use crate::cli_util::WorkspaceCommandHelper;
use crate::command_error::CommandError;
use crate::command_error::config_error;
use crate::command_error::user_error;
use crate::command_error::user_error_with_message;
use crate::ui::Ui;

/// Publish subtrees of this repository to repositories of their own
#[derive(clap::Subcommand, Clone, Debug)]
pub enum ViewsCommand {
    Add(add::ViewsAddArgs),
    Anchor(anchor::ViewsAnchorArgs),
    Check(check::ViewsCheckArgs),
    Fetch(fetch::ViewsFetchArgs),
    Patches(patches::ViewsPatchesArgs),
    Prompt(prompt::ViewsPromptArgs),
    Push(push::ViewsPushArgs),
    Status(status::ViewsStatusArgs),
    Tree(tree::ViewsTreeArgs),
}

pub async fn cmd_views(
    ui: &mut Ui,
    command: &CommandHelper,
    subcommand: &ViewsCommand,
) -> Result<(), CommandError> {
    match subcommand {
        ViewsCommand::Add(args) => add::cmd_views_add(ui, command, args).await,
        ViewsCommand::Anchor(args) => anchor::cmd_views_anchor(ui, command, args).await,
        ViewsCommand::Check(args) => check::cmd_views_check(ui, command, args).await,
        ViewsCommand::Fetch(args) => fetch::cmd_views_fetch(ui, command, args).await,
        ViewsCommand::Patches(args) => patches::cmd_views_patches(ui, command, args).await,
        ViewsCommand::Prompt(args) => prompt::cmd_views_prompt(ui, command, args).await,
        ViewsCommand::Push(args) => push::cmd_views_push(ui, command, args).await,
        ViewsCommand::Status(args) => status::cmd_views_status(ui, command, args).await,
        ViewsCommand::Tree(args) => tree::cmd_views_tree(ui, command, args).await,
    }
}

/// One entry of the `views` config table, checked.
#[derive(Clone, Debug)]
struct ViewConfig {
    /// The table key, used to name the view on the command line and in output.
    name: String,
    /// Path prefix in this repository that the view is of.
    path: String,
    /// URL the view is published to. Any URL `git push` accepts.
    remote: String,
    /// The published repository's own default branch. This is the base a pull
    /// request is opened against, and the one branch `jj views push` will not
    /// write without being asked twice.
    branch: String,
    /// Checked source-to-published starting point for incremental derivation.
    anchor: Option<jj_views::DeriveAnchor>,
    /// Recreate the anchor as a parentless commit from its source snapshot.
    root_anchor: bool,
    /// A read-only source whose history can be lifted into this view.
    upstream: Option<ViewEndpoint>,
}

#[derive(Clone, Debug)]
struct ViewEndpoint {
    remote: String,
    branch: String,
}

impl ViewConfig {
    /// The filter that derives this view.
    ///
    /// Neither the elision rule nor the semantics version is configurable, and
    /// that is deliberate: the two are what decide the hashes, a published
    /// history cannot be un-published, and the values here are the ones that
    /// preserve identity. `jj-views derive` defaults to the same pair, so the
    /// command and the tool agree on what a view is.
    fn filter(&self) -> Result<Filter, CommandError> {
        Filter::prefix(&self.path)
            .map(|filter| filter.semantics(Semantics::V2).elide(Elide::Unchanged))
            .map_err(|err| config_error(format!("In `views.{}`: {err}", self.name)))
    }
}

/// Deserialization shape of a `views.<name>` table.
#[derive(Clone, Debug, serde::Deserialize)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
struct RawViewConfig {
    path: String,
    remote: String,
    branch: String,
    anchor: Option<RawViewAnchor>,
    upstream_remote: Option<String>,
    upstream_branch: Option<String>,
}

#[derive(Clone, Debug, serde::Deserialize)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
struct RawViewAnchor {
    source: String,
    view: String,
    #[serde(default)]
    root: bool,
}

#[derive(Debug, serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct ViewManifest {
    views: BTreeMap<String, RawViewConfig>,
}

const VIEW_MANIFEST_FILE_NAME: &str = ".jj-views.toml";

/// Paths whose change alone never makes a commit mixed.
///
/// Only the manifest, and only because adopting a view writes it in the same
/// commit that vendors the tree. Counting it would make `--allow-mixed` the
/// habitual answer to the most ordinary views operation, and a flag everybody
/// passes every time is not a gate. A commit that changes the manifest *and*
/// something else outside the view is still refused, which is the case the
/// refusal exists for.
fn exempt_paths() -> Vec<bstr::BString> {
    vec![bstr::BString::from(VIEW_MANIFEST_FILE_NAME)]
}

/// Parses the `views` config table.
///
/// The manifest is read from the working-copy commit, not from the files on
/// disk. It is repository state: which subtrees this repository publishes does
/// not depend on which files a particular checkout happens to materialize, and
/// every command here already works against the commit. Reading the checkout
/// made a sparse working copy that leaves `.jj-views.toml` unmaterialized
/// indistinguishable from a repository that declares no views at all --
/// `jj views tree` printed the workspace root and nothing under it, and the
/// rest reported `No views are configured`. The same applies one level down:
/// a checkout can materialize the root manifest and not a nested one.
///
/// Every entry is checked here rather than when it is reached, so a typo in the
/// fourth view is reported before the first one is pushed anywhere.
async fn get_views_config(
    workspace_command: &WorkspaceCommandHelper,
) -> Result<Vec<ViewConfig>, CommandError> {
    if let Some(tree) = working_copy_tree(workspace_command).await?
        && let Some(views) = manifest_views_config(&tree, "").await?
    {
        return Ok(views);
    }

    // Sorted so that both the error reported for a bad table and the order the
    // views are pushed in are the same on every machine.
    let settings = workspace_command.settings();
    settings
        .table_keys("views")
        .sorted()
        .map(|name| {
            let raw: RawViewConfig = settings.get(["views", name])?;
            check_view_config(name.to_owned(), raw)
        })
        .try_collect()
}

/// The tree every manifest is read from.
///
/// `None` in a workspace that has no working-copy commit, which is the one
/// case with no commit to read and where the settings table is all there is.
async fn working_copy_tree(
    workspace_command: &WorkspaceCommandHelper,
) -> Result<Option<MergedTree>, CommandError> {
    let Some(id) = workspace_command.get_wc_commit_id() else {
        return Ok(None);
    };
    let commit = workspace_command
        .repo()
        .store()
        .get_commit_async(id)
        .await?;
    Ok(Some(commit.tree()))
}

/// Parses the manifest under `dir`, if the tree carries one there.
///
/// The tree-only half of [`get_views_config`]: `dir` is `""` for the
/// repository root, and a view's path for a nested manifest, which lives
/// inside that view's own content and has no settings table to fall back to.
async fn manifest_views_config(
    tree: &MergedTree,
    dir: &str,
) -> Result<Option<Vec<ViewConfig>>, CommandError> {
    let path = manifest_path(dir)?;
    let Some(body) = read_manifest(tree, &path).await? else {
        return Ok(None);
    };
    let manifest: ViewManifest = toml::from_str(&body).map_err(|err| {
        user_error_with_message(
            format!("Could not parse {}", path.as_internal_file_string()),
            err,
        )
    })?;
    manifest
        .views
        .into_iter()
        .map(|(name, raw)| check_view_config(name, raw))
        .collect::<Result<_, _>>()
        .map(Some)
}

/// Where a manifest sits: at the repository root for `""`, and inside the
/// directory otherwise.
fn manifest_path(dir: &str) -> Result<RepoPathBuf, CommandError> {
    let dir = dir.trim_matches('/');
    let value = if dir.is_empty() {
        VIEW_MANIFEST_FILE_NAME.to_owned()
    } else {
        format!("{dir}/{VIEW_MANIFEST_FILE_NAME}")
    };
    RepoPathBuf::from_internal_string(value)
        .map_err(|err| config_error(format!("Bad manifest path: {err}")))
}

/// The manifest blob at `path`, or `None` when the tree holds nothing there.
async fn read_manifest(tree: &MergedTree, path: &RepoPath) -> Result<Option<String>, CommandError> {
    let display = path.as_internal_file_string();
    let Ok(value) = tree.path_value(path).await?.into_resolved() else {
        return Err(user_error(format!(
            "{display} is conflicted in the working-copy commit"
        ))
        .hinted("Resolve the conflict before running a `views` command."));
    };
    let id = match value {
        None => return Ok(None),
        Some(TreeValue::File { id, .. }) => id,
        Some(_) => {
            return Err(user_error(format!(
                "{display} is not a regular file in the working-copy commit"
            )));
        }
    };
    let mut reader = tree.store().read_file(path, &id).await?;
    let mut body = Vec::new();
    futures::AsyncReadExt::read_to_end(&mut reader, &mut body)
        .await
        .map_err(|err| user_error_with_message(format!("Could not read {display}"), err))?;
    String::from_utf8(body)
        .map(Some)
        .map_err(|err| user_error(format!("{display} is not UTF-8: {err}")))
}

fn check_view_config(name: String, raw: RawViewConfig) -> Result<ViewConfig, CommandError> {
    let root_anchor = raw.anchor.as_ref().is_some_and(|anchor| anchor.root);
    let anchor = raw
        .anchor
        .map(|anchor| -> Result<_, CommandError> {
            let source = parse_anchor_id(&name, "source", &anchor.source)?;
            let view = parse_anchor_id(&name, "view", &anchor.view)?;
            Ok(jj_views::DeriveAnchor { source, view })
        })
        .transpose()?;
    let upstream = match (raw.upstream_remote, raw.upstream_branch) {
        (Some(remote), Some(branch)) => Some(ViewEndpoint { remote, branch }),
        (None, None) => None,
        _ => {
            return Err(config_error(format!(
                "`views.{name}.upstream-remote` and `views.{name}.upstream-branch` must be set \
                 together"
            )));
        }
    };
    let config = ViewConfig {
        name: name.clone(),
        path: raw.path,
        remote: raw.remote,
        branch: raw.branch,
        anchor,
        root_anchor,
        upstream,
    };
    // The name becomes a file name (`record`) and a ref name (the tracking
    // ref), so a separator would let one view write outside both namespaces.
    if name.contains(['/', '\\']) {
        return Err(config_error(format!(
            "View name {name} cannot contain a path separator"
        )));
    }
    if config.remote.is_empty() {
        return Err(config_error(format!("`views.{name}.remote` is empty")));
    }
    if config.branch.is_empty() {
        return Err(config_error(format!("`views.{name}.branch` is empty")));
    }
    if let Some(upstream) = &config.upstream
        && (upstream.remote.is_empty() || upstream.branch.is_empty())
    {
        return Err(config_error(format!(
            "The upstream endpoint for `views.{name}` has an empty value"
        )));
    }
    config.filter()?;
    Ok(config)
}

fn parse_anchor_id(name: &str, field: &str, value: &str) -> Result<gix::ObjectId, CommandError> {
    if !matches!(value.len(), 40 | 64) {
        return Err(config_error(format!(
            "`views.{name}.anchor.{field}` must be a full 40 or 64 digit Git object id"
        )));
    }
    gix::ObjectId::from_hex(value.as_bytes()).map_err(|err| {
        config_error(format!(
            "`views.{name}.anchor.{field}` is not a full Git object id: {err}"
        ))
    })
}

/// Resolves the names on the command line against the configured views.
fn select_views<'a>(
    configured: &'a [ViewConfig],
    names: &[String],
) -> Result<Vec<&'a ViewConfig>, CommandError> {
    if configured.is_empty() {
        return Err(config_error("No `views` are configured")
            .hinted("Add a `[views.NAME]` table with `path`, `remote` and `branch` keys."));
    }
    if names.is_empty() {
        return Ok(configured.iter().collect());
    }
    names
        .iter()
        .map(|name| {
            configured
                .iter()
                .find(|view| view.name == *name)
                .ok_or_else(|| {
                    user_error(format!("No such view: {name}")).hinted(format!(
                        "Configured views are: {}",
                        configured.iter().map(|view| &view.name).join(", ")
                    ))
                })
        })
        .try_collect()
}

/// Where each view's upstream tip is remembered between fetches.
///
/// This is the view equivalent of a remote-tracking bookmark: it records what
/// the published repository said, separately from what this repository has done
/// with it. `jj views status` reads it without going to the network.
const UPSTREAM_REF_NAMESPACE: &str = "refs/jj/views-upstream/";
const READ_ONLY_UPSTREAM_REF_NAMESPACE: &str = "refs/jj/views-read-only-upstream/";
const ANCHOR_REF_NAMESPACE: &str = "refs/jj/views-anchor/";
const ANCHOR_SOURCE_REF_NAMESPACE: &str = "refs/jj/views-anchor-source/";
const ANCHOR_REVISION_REF_NAMESPACE: &str = "refs/jj/views-anchor-revision/";

/// Where one view's published repository stands, relative to this one.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Position {
    /// The published history is integrated and this repository adds no view
    /// content, though it may add topology.
    Current,
    /// This repository holds everything the published repository does, and view
    /// commits of its own. `jj views push` is the move.
    LocalAhead,
    /// The published repository is strictly ahead.
    FastForward,
    /// Both sides have view commits the other does not.
    Diverged,
}

/// What one view's two sides turned out to be.
///
/// Every command here needs the same comparison and all of them must agree on
/// it: whether published history is integrated and whether local topology adds
/// content decide both status and push. Two implementations would be two
/// answers.
struct Survey<'view> {
    view: &'view ViewConfig,
    remote: &'view str,
    branch: &'view str,
    /// What the published repository's branch points at.
    upstream: gix::ObjectId,
    /// The view tip this repository derives to. `None` when nothing under the
    /// prefix exists in the anchor's ancestry, which is a view never imported.
    derived: Option<gix::ObjectId>,
    /// Published view commits this repository has not integrated, parents
    /// first.
    incoming: Vec<gix::ObjectId>,
    /// Published commits this repository holds as commits the view drops.
    ///
    /// Counted rather than ignored because it is the difference between a view
    /// that is behind and one that only looks behind. `git rev-list
    /// derived..upstream` counts these, no fetch can ever remove them from that
    /// count, and a reader without the number concludes the fetch did nothing.
    elided: usize,
    /// View commits this repository has that the published repository does not.
    ahead: usize,
    position: Position,
}

/// Whether to ask the published repository where it is, or use what it last
/// said.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Freshness {
    /// Fetch the published repository's branch into the tracking ref first.
    Fetch,
    /// Read the tracking ref the last fetch left, and touch no network.
    AsOfLastFetch,
}

/// Positions one view against its published repository.
///
/// Reading is all this does. It writes the tracking ref under
/// [`UPSTREAM_REF_NAMESPACE`] when asked to fetch and nothing else, so every
/// view can be positioned before any of them is acted on and a problem in the
/// third view is reported with the first two still untouched.
fn survey<'view>(
    git: &Git,
    repo: &mut gix::Repository,
    view: &'view ViewConfig,
    anchor: &gix::ObjectId,
    read_only_upstream: bool,
    freshness: Freshness,
    cache: &mut Cache,
) -> Result<Survey<'view>, CommandError> {
    let filter = view.filter()?;
    let (remote, branch, namespace) = if read_only_upstream {
        let endpoint = view.upstream.as_ref().ok_or_else(|| {
            user_error(format!(
                "The {} view has no read-only upstream endpoint",
                view.name
            ))
            .hinted(
                "Add `upstream-remote` and `upstream-branch` to its manifest entry, or drop \
                 --upstream.",
            )
        })?;
        (
            endpoint.remote.as_str(),
            endpoint.branch.as_str(),
            READ_ONLY_UPSTREAM_REF_NAMESPACE,
        )
    } else {
        (
            view.remote.as_str(),
            view.branch.as_str(),
            UPSTREAM_REF_NAMESPACE,
        )
    };
    let tracking = format!("{namespace}{}", view.name);
    if freshness == Freshness::Fetch {
        *repo = git
            .fetch(remote, branch, &tracking)
            .map_err(|err| user_error(format!("Could not fetch the {} view: {err}", view.name)))?;
    }
    let upstream = resolve(repo, &tracking)?.ok_or_else(|| match freshness {
        Freshness::Fetch => user_error(format!(
            "No branch {} at {}, so the {} view has nothing to fetch",
            branch, remote, view.name
        )),
        Freshness::AsOfLastFetch => {
            user_error(format!("The {} view has never been fetched", view.name))
                .hinted("Run `jj views fetch` first, or drop --no-fetch.")
        }
    })?;

    seed_view_anchor(repo, view, anchor, cache)?;
    let derived_tip =
        jj_views::derive_tip(repo, anchor, &filter, cache).map_err(|err| lift_error(view, err))?;
    if derived_tip.is_none() {
        return Err(user_error(format!(
            "The {} view at {} has never been imported into {}",
            view.name, view.path, anchor
        ))
        .hinted(format!(
            "Seed {} with `jj-views import`, run `jj git import`, then merge the imported lineage \
             into the bookmark before fetching.",
            view.path
        )));
    }
    if derived_tip == Some(upstream) {
        return Ok(Survey {
            view,
            remote,
            branch,
            upstream,
            derived: derived_tip,
            incoming: Vec::new(),
            elided: 0,
            ahead: 0,
            position: Position::Current,
        });
    }

    cache.discard_filter(&filter);
    seed_view_anchor(repo, view, anchor, cache)?;
    let integrated = match view.anchor {
        Some(view_anchor) => {
            jj_views::verify::integrated_after_anchor(repo, anchor, view_anchor, &filter, cache)
        }
        None => jj_views::verify::integrated(repo, anchor, &filter, cache),
    }
    .map_err(|err| lift_error(view, err))?;
    let derived =
        jj_views::derive(repo, anchor, &filter, cache).map_err(|err| lift_error(view, err))?;

    let published_integrated = integrated.contains(&upstream);
    let (same_content, same_shape) = match derived {
        Some(tip) => {
            let local = commit_shape(repo, tip)?;
            let published = commit_shape(repo, upstream)?;
            (local.0 == published.0, local == published)
        }
        None => (false, false),
    };
    // Rewriting a host commit can derive a sibling with changed metadata. Equal
    // trees and parents prove the view history did not change. Equal trees with
    // different parents still take the exhaustive path, which catches a change
    // followed by its revert.
    let content_current = (published_integrated && same_content) || same_shape;

    let ancestry = match view.anchor {
        Some(anchor) => jj_views::verify::ancestry_after(repo, &upstream, &anchor.view),
        None => jj_views::verify::ancestry(repo, &upstream),
    }
    .map_err(|err| lift_error(view, err))?;
    let mut reachable: HashSet<gix::ObjectId> = ancestry.iter().copied().collect();
    if let Some(anchor) = view.anchor {
        reachable.insert(anchor.view);
    }

    let incoming: Vec<gix::ObjectId> = if same_shape {
        Vec::new()
    } else {
        ancestry
            .iter()
            .copied()
            .filter(|id| !integrated.contains(id))
            .collect()
    };
    // Only what raw ancestry actually calls behind. An elided commit the view
    // tip already reaches is not in `derived..upstream`, has confused nobody,
    // and counting it would make this number grow with every fetch until it
    // meant nothing.
    let behind: Vec<gix::ObjectId> = match derived {
        Some(tip) => {
            let mut shared: HashSet<gix::ObjectId> = match view.anchor {
                Some(anchor) => jj_views::verify::ancestry_after(repo, &tip, &anchor.view),
                None => jj_views::verify::ancestry(repo, &tip),
            }
            .map_err(|err| lift_error(view, err))?
            .into_iter()
            .collect();
            if let Some(anchor) = view.anchor {
                shared.insert(anchor.view);
            }
            ancestry
                .iter()
                .copied()
                .filter(|id| !shared.contains(id))
                .collect()
        }
        None => ancestry.clone(),
    };
    let elided = behind
        .iter()
        .filter(|id| !integrated.derived.contains(*id) && integrated.elided.contains(*id))
        .count();
    let unpublished = integrated
        .derived
        .iter()
        .filter(|id| !reachable.contains(*id))
        .count();
    // A local view tip the upstream history does not contain is a second line
    // of development. Equal content is classified as current below.
    let local_ahead = derived.is_some_and(|tip| !reachable.contains(&tip));

    let (position, ahead) = match (content_current, published_integrated, local_ahead) {
        (true, _, _) => (Position::Current, 0),
        (false, true, false) => (Position::Current, unpublished),
        (false, true, true) => (Position::LocalAhead, unpublished),
        (false, false, true) => (Position::Diverged, unpublished),
        (false, false, false) => (Position::FastForward, unpublished),
    };
    Ok(Survey {
        view,
        remote,
        branch,
        upstream,
        derived,
        incoming,
        elided,
        ahead,
        position,
    })
}

fn require_tracking_refs(
    repo: &gix::Repository,
    views: &[&ViewConfig],
    read_only_upstream: bool,
) -> Result<(), CommandError> {
    let namespace = if read_only_upstream {
        READ_ONLY_UPSTREAM_REF_NAMESPACE
    } else {
        UPSTREAM_REF_NAMESPACE
    };
    for view in views {
        let tracking = format!("{namespace}{}", view.name);
        if resolve(repo, &tracking)?.is_none() {
            return Err(
                user_error(format!("The {} view has never been fetched", view.name))
                    .hinted("Run `jj views fetch` first, or drop --no-fetch."),
            );
        }
    }
    Ok(())
}

/// The bookmark a view's history lives on, and the commit it points at.
///
/// This is both the revision every view is derived from and, for the commands
/// that write, the ref that moves.
fn anchor(
    workspace_command: &WorkspaceCommandHelper,
    bookmark: &str,
    operation: &str,
) -> Result<(RefNameBuf, gix::ObjectId), CommandError> {
    let name = RefNameBuf::from(bookmark.to_owned());
    let target = workspace_command
        .repo()
        .view()
        .get_local_bookmark(&name)
        .as_normal()
        .cloned()
        .ok_or_else(|| {
            user_error(format!(
                "Bookmark {bookmark} does not exist or is conflicted, so there is nothing to \
                 derive the views from"
            ))
            .hinted("Use --bookmark to name the bookmark the view's history belongs on.")
        })?;
    // A commit id is only a Git object id on a backend that keeps Git objects.
    // Every caller checks the store first, so reaching this with something else
    // is a caller's ordering bug rather than a user's; it still reports rather
    // than panicking, because the width of a commit id is a backend's choice
    // and a new backend must not be able to abort the process by making one.
    let target = gix::ObjectId::try_from(target.as_bytes())
        .map_err(|_| needs_git_objects(workspace_command, operation))?;
    Ok((name, target))
}

/// What `bookmark` points at here, in this backend's own commit ids.
///
/// The generic sibling of [`anchor`]. Deliberately not a `gix::ObjectId`: on a
/// backend that keeps no Git objects this commit is not one, and rendering it
/// as though it were invites a reader to compare it with a published Git hash
/// it has no relationship to.
fn local_bookmark_commit(
    workspace_command: &WorkspaceCommandHelper,
    bookmark: &str,
) -> Option<CommitId> {
    workspace_command
        .repo()
        .view()
        .get_local_bookmark(&RefNameBuf::from(bookmark.to_owned()))
        .as_normal()
        .cloned()
}

/// Opens the Git store the derived objects live in, and `git` pointed at it,
/// or `None` when this repository keeps no Git objects at all.
///
/// A view's commits are the published Git repository's commits, hash for hash,
/// so deriving one reads the source history as Git commit and tree objects. A
/// backend that stores commits under its own hashes has none to read. That is
/// a property of the backend rather than a fault in the checkout, and the
/// commands that only read the manifest keep working across it, so the absence
/// is returned rather than raised and each caller says what it wanted.
fn try_open_store(
    workspace_command: &WorkspaceCommandHelper,
) -> Result<Option<(Git, gix::Repository)>, CommandError> {
    let settings = workspace_command.settings();
    let Ok(git_backend) = jj_lib::git::get_git_backend(workspace_command.repo().store()) else {
        return Ok(None);
    };
    let git = Git {
        executable: jj_lib::git::GitSettings::from_settings(settings)?.executable_path,
        git_dir: git_backend.git_repo_path().to_owned(),
    };
    let repo = git_backend.git_repo();
    Ok(Some((git, repo)))
}

/// The Git store, or why `jj views <operation>` cannot run in this repository.
fn open_store(
    workspace_command: &WorkspaceCommandHelper,
    operation: &str,
) -> Result<(Git, gix::Repository), CommandError> {
    match try_open_store(workspace_command)? {
        Some(store) => Ok(store),
        None => Err(needs_git_objects(workspace_command, operation)),
    }
}

/// The `gix` handle alone, for a command that reads objects and runs no `git`.
///
/// Separate from [`open_store`] so a command that never shells out is not
/// failed by an unreadable `git.executable-path`, which is a different problem
/// with a different fix and would be reported here as this one.
fn open_git_repo(
    workspace_command: &WorkspaceCommandHelper,
    operation: &str,
) -> Result<gix::Repository, CommandError> {
    match jj_lib::git::get_git_backend(workspace_command.repo().store()) {
        Ok(git_backend) => Ok(git_backend.git_repo()),
        Err(_) => Err(needs_git_objects(workspace_command, operation)),
    }
}

/// Names the capability that is missing, and the backend that does not have
/// it, instead of reporting the repository as broken.
///
/// `The repo is not backed by a Git repo` is true and useless: it reads as a
/// checkout to go repair, it says nothing about which of nine subcommands it
/// came from, and it gives a reader no way to tell "this backend cannot do
/// this" from "you are standing in the wrong directory". Every sentence here
/// exists because a reader of the old one had to guess it.
fn needs_git_objects(workspace_command: &WorkspaceCommandHelper, operation: &str) -> CommandError {
    let backend = workspace_command.repo().store().backend().name();
    user_error(format!(
        "`jj views {operation}` needs this repository's history as Git objects, and the {backend} \
         backend does not keep them"
    ))
    // No article before the backend's name: it is whatever a backend author
    // called it, so `a ix repository` is one grammar bug away at all times.
    .hinted(
        "A view's commits are the published Git repository's commits, hash for hash, so deriving \
         one reads Git commit and tree objects. `jj views tree`, `jj views status` and `jj views \
         prompt` answer from the manifest and work here; `add`, `anchor`, `check`, `fetch`, \
         `patches` and `push` need a Git-backed clone of this repository.",
    )
}

/// Checks the selected views' endpoints and anchors before the command reaches
/// the network or mutates repository state.
///
/// Selected, not configured. Seeding an anchor reads the commit object it
/// names, so validating every entry in the manifest made every view's health a
/// precondition of every other view's push: one manifest entry whose anchor had
/// rotted -- a commit its published branch no longer reaches and no endpoint
/// will serve -- failed `jj views push <one healthy view>` before that view was
/// looked at. A view nobody named is a view this command has no business
/// reading. `jj views tree` and an unqualified `jj views status` still report
/// on all of them, which is where a whole-manifest answer belongs.
fn validate_views(
    git: &Git,
    repo: &gix::Repository,
    selected: &[&ViewConfig],
    revision: &gix::ObjectId,
    cache: &mut Cache,
) -> Result<(), CommandError> {
    validate_endpoints(git, selected)?;
    for view in selected {
        seed_view_anchor(repo, view, revision, cache)?;
    }
    Ok(())
}

fn validate_endpoints(git: &Git, selected: &[&ViewConfig]) -> Result<(), CommandError> {
    for view in selected {
        git.check_ref_format(&view.branch)?;
        if let Some(upstream) = &view.upstream {
            git.check_ref_format(&upstream.branch)?;
        }
    }
    Ok(())
}

fn seed_view_anchor(
    repo: &gix::Repository,
    view: &ViewConfig,
    revision: &gix::ObjectId,
    cache: &mut Cache,
) -> Result<(), CommandError> {
    if let Some(anchor) = view.anchor {
        let recorded_view = resolve(repo, &format!("{ANCHOR_REF_NAMESPACE}{}", view.name))?;
        let recorded_source =
            resolve(repo, &format!("{ANCHOR_SOURCE_REF_NAMESPACE}{}", view.name))?;
        let recorded_revision = resolve(
            repo,
            &format!("{ANCHOR_REVISION_REF_NAMESPACE}{}", view.name),
        )?;
        let seed_hint = |err: jj_views::Error| {
            lift_error(view, err).hinted(format!(
                "Seeding the {} anchor ({} -> {}) failed. If the anchor commit is not in this \
                 repository yet, `jj views anchor` fetches it from the published or upstream \
                 endpoint and validates it before anything else runs.",
                view.name, anchor.source, anchor.view
            ))
        };
        if recorded_view == Some(anchor.view)
            && recorded_source == Some(anchor.source)
            && recorded_revision == Some(*revision)
        {
            cache
                .seed_anchor_after_ancestry_check(repo, &view.filter()?, anchor)
                .map_err(seed_hint)?;
        } else {
            cache
                .seed_anchor(repo, revision, &view.filter()?, anchor)
                .map_err(seed_hint)?;
        }
    }
    Ok(())
}

fn lift_error(view: &ViewConfig, err: jj_views::Error) -> CommandError {
    user_error_with_message(format!("Could not lift the {} view", view.name), err)
}

/// What `name` points at, or `None` when there is no such ref.
///
/// An absent ref and an unreadable one are different situations -- one is a
/// view nobody has fetched, the other is a broken store -- so only the first
/// becomes `None`.
fn resolve(repo: &gix::Repository, name: &str) -> Result<Option<gix::ObjectId>, CommandError> {
    let mut reference = match repo.find_reference(name) {
        Ok(reference) => reference,
        Err(gix::reference::find::existing::Error::NotFound { .. }) => return Ok(None),
        Err(err) => {
            return Err(user_error_with_message(
                format!("Could not read {name}"),
                err,
            ));
        }
    };
    Ok(Some(
        reference
            .peel_to_id()
            .map_err(|err| user_error_with_message(format!("Could not resolve {name}"), err))?
            .detach(),
    ))
}

fn commit_shape(
    repo: &gix::Repository,
    commit: gix::ObjectId,
) -> Result<(gix::ObjectId, Vec<gix::ObjectId>), CommandError> {
    let raw = repo
        .find_object(commit)
        .map_err(|err| user_error_with_message(format!("Could not read commit {commit}"), err))?
        .detach()
        .data;
    gix::objs::CommitRef::from_bytes(&raw, repo.object_hash())
        .map(|commit| (commit.tree(), commit.parents().collect()))
        .map_err(|err| user_error_with_message(format!("Could not parse commit {commit}"), err))
}

fn commit_tree(
    repo: &gix::Repository,
    commit: gix::ObjectId,
) -> Result<gix::ObjectId, CommandError> {
    Ok(commit_shape(repo, commit)?.0)
}

/// Lifts a view's incoming commits into this repository.
///
/// Returns the counterpart of the published tip. Every commit is lifted onto
/// the same `onto`: the cache carries each commit's real parents, and `onto`
/// only positions one whose ancestry this repository does not know.
///
/// For a diverged view that means `onto` is not used at all: every incoming
/// commit's parent is either another incoming commit or the commit the two
/// sides last agreed on, which this repository has. So the result lands beside
/// the local history rather than on top of it, and lifting the same commits
/// again produces the same ids however the bookmark has moved in between. That
/// idempotence is what lets `jj views fetch` bring a diverged view's commits in
/// on every run without leaving a second copy.
fn lift(
    repo: &gix::Repository,
    survey: &Survey,
    onto: &gix::ObjectId,
    cache: &mut Cache,
) -> Result<gix::ObjectId, CommandError> {
    let filter = survey.view.filter()?;
    let mut head = None;
    for commit in &survey.incoming {
        let id = jj_views::unfilter(repo, commit, onto, &filter, cache)
            .map_err(|err| lift_error(survey.view, err))?;
        if *commit == survey.upstream {
            head = Some(id);
        }
    }
    head.ok_or_else(|| {
        user_error(format!(
            "The {} view's tip was not lifted",
            survey.view.name
        ))
    })
}

/// The working-copy commit and its first parent, when there is one.
///
/// Absent for a workspace with no working copy, and for a working copy on a
/// root commit, where "what you have edited" has no meaning.
fn working_copy_range(
    workspace_command: &WorkspaceCommandHelper,
) -> Option<(gix::ObjectId, gix::ObjectId)> {
    let wc_id = workspace_command.get_wc_commit_id()?;
    let commit = workspace_command.repo().store().get_commit(wc_id).ok()?;
    let parent = commit.parent_ids().first()?.clone();
    // Absent, not fatal, when these ids are not Git object ids: this helper
    // already answers `None` for the cases where the range has no meaning, and
    // a backend that keeps no Git objects is one more of them.
    Some((
        gix::ObjectId::try_from(parent.as_bytes()).ok()?,
        gix::ObjectId::try_from(wc_id.as_bytes()).ok()?,
    ))
}

/// How many of the incoming commits touch a file the working copy has changed.
///
/// This is the half of a fetch report a person acts on: a hundred incoming
/// commits matter less than the three that touch a file they are mid-edit on.
///
/// Best effort on purpose: it is a heads-up printed after work that already
/// succeeded, so a git invocation that fails here must not turn a fetch that
/// worked into a command that failed. It reports nothing rather than guessing.
fn overlap_with_working_copy(
    git: &Git,
    view: &ViewConfig,
    incoming: &[gix::ObjectId],
    edited: Option<&(gix::ObjectId, gix::ObjectId)>,
) -> Option<String> {
    let (parent, wc) = edited?;
    let prefix = view.path.trim_end_matches('/');
    let mine: HashSet<String> = git
        .changed_under(*parent, *wc, prefix)
        .ok()?
        .into_iter()
        // The view's commits name paths relative to the subtree root, so the
        // working copy's host-relative paths have to lose the prefix to match.
        .filter_map(|path| {
            path.strip_prefix(prefix)
                .map(|rest| rest.trim_start_matches('/').to_owned())
        })
        .collect();
    if mine.is_empty() {
        return None;
    }
    let colliding = git
        .touched_paths(incoming)
        .ok()?
        .into_iter()
        .filter(|(_, paths)| paths.iter().any(|path| mine.contains(path)))
        .count();
    if colliding == 0 {
        return Some(format!(
            "None of them touch the {} files you have edited here.",
            mine.len()
        ));
    }
    Some(format!(
        "{colliding} of them touch files you have edited here."
    ))
}

/// The line that explains a count nothing else can explain.
///
/// A published commit that arrived and that the view then drops stays inside
/// `git rev-list <view tip>..<upstream>` forever, so anyone checking with raw
/// ancestry sees a view that is behind and a fetch that did nothing about it.
/// Saying the number is the difference between that and a bug report.
fn elided_note(count: usize) -> String {
    let (commits, they) = if count == 1 {
        ("commit is", "it")
    } else {
        ("commits are", "them")
    };
    format!(
        "{count} published {commits} here and elided from the view, so raw ancestry counts {they} \
         as behind."
    )
}

/// `1 commit` or `n commits`.
fn commits(count: usize) -> String {
    if count == 1 {
        "1 commit".to_owned()
    } else {
        format!("{count} commits")
    }
}

/// What a diverged view's report says, once its commits are here.
///
/// jj has no `merge` command -- a merge is `jj new A B` -- and it does have
/// `rebase`, so a `jj views` verb for either would be a second spelling of
/// something the user already has. This is the whole of the integration
/// surface instead: bring the published commits in as commits, name the
/// revision, and let the user pick the jj command they already understand the
/// consequences of.
///
/// The warning is not decoration. Rebasing the lifted lineage onto the local
/// one re-derives every published commit to a new hash, so the published
/// repository ends up holding the content without the commits, and neither
/// side can see that until somebody's fetch fails.
fn diverged_report(survey: &Survey, head: gix::ObjectId, bookmark: &str) -> Vec<String> {
    let local = survey
        .derived
        .map(|id| id.to_string())
        .unwrap_or_else(|| "nothing".to_owned());
    vec![
        format!(
            "This repository has {local} under {}, and {} has {}",
            survey.view.path, survey.remote, survey.upstream
        ),
        format!("The published history is here as {head}, beside {bookmark}"),
        format!(
            "`jj new {bookmark} {head}` integrates it. Do not rebase it onto {bookmark}: that \
             rewrites commits {} has already handed out.",
            survey.remote
        ),
    ]
}

/// `git`, pointed at the store the derived objects live in.
struct Git {
    executable: PathBuf,
    git_dir: PathBuf,
}

struct ShallowFetch {
    directory: tempfile::TempDir,
    tip: gix::ObjectId,
    anchor: gix::ObjectId,
    anchor_commit: Vec<u8>,
}

fn validate_shallow_history(
    output: &str,
    depth: usize,
    anchor: gix::ObjectId,
) -> Result<(), String> {
    let rows: Vec<Vec<gix::ObjectId>> = output
        .lines()
        .map(|line| {
            line.split_ascii_whitespace()
                .map(|id| {
                    gix::ObjectId::from_hex(id.as_bytes())
                        .map_err(|err| format!("Git returned an invalid object id {id}: {err}"))
                })
                .collect()
        })
        .collect::<Result<_, _>>()?;
    if rows.iter().any(|row| row.first() == Some(&anchor)) {
        Ok(())
    } else {
        Err(format!(
            "fetched {} commits at depth {depth} without reaching anchor {anchor}",
            rows.len()
        ))
    }
}

/// Whether `repo` is a genuinely partial (promisor) clone: a configured
/// promisor remote, the `extensions.partialClone` marker, or a promisor-marked
/// pack. Only such a repository may treat a missing anchor leaf blob as
/// promised-but-absent; an ordinary repository missing one is corrupt and keeps
/// failing closed.
fn is_promisor_repository(repo: &gix::Repository) -> bool {
    let config = repo.config_snapshot();
    if config.string("extensions.partialClone").is_some() {
        return true;
    }
    if repo.remote_names().iter().any(|name| {
        // Bound rather than passed inline: gix implements AsKey for borrowed
        // strings, not for an owned String.
        let key = format!("remote.{name}.promisor");
        config.boolean(&key).unwrap_or(false)
    }) {
        return true;
    }
    match fs::read_dir(repo.git_dir().join("objects").join("pack")) {
        Ok(entries) => entries.filter_map(Result::ok).any(|entry| {
            entry
                .path()
                .extension()
                .is_some_and(|extension| extension == "promisor")
        }),
        Err(_) => false,
    }
}

fn install_anchor_objects(
    source: &gix::Repository,
    destination: &gix::Repository,
    prepared: &ShallowFetch,
) -> Result<(), String> {
    let promised_leaves_allowed = is_promisor_repository(destination);
    let written = destination
        .objects
        .write_buf(gix::objs::Kind::Commit, &prepared.anchor_commit)
        .map_err(|err| format!("could not write anchor commit: {err}"))?;
    if written != prepared.anchor {
        return Err(format!(
            "wrote anchor commit as {written}; expected {}",
            prepared.anchor
        ));
    }
    let commit = gix::objs::CommitRef::from_bytes(&prepared.anchor_commit, source.object_hash())
        .map_err(|err| format!("could not decode anchor commit: {err}"))?;
    let mut trees = vec![commit.tree()];
    let mut seen = HashSet::new();
    while let Some(tree) = trees.pop() {
        if !seen.insert(tree) {
            continue;
        }
        let raw = source
            .find_object(tree)
            .map_err(|err| format!("could not read anchor tree {tree}: {err}"))?
            .detach()
            .data;
        let written = destination
            .objects
            .write_buf(gix::objs::Kind::Tree, &raw)
            .map_err(|err| format!("could not write anchor tree {tree}: {err}"))?;
        if written != tree {
            return Err(format!("wrote anchor tree as {written}; expected {tree}"));
        }
        let decoded = gix::objs::TreeRef::from_bytes(&raw, source.object_hash())
            .map_err(|err| format!("could not decode anchor tree {tree}: {err}"))?;
        for entry in decoded.entries {
            match entry.mode.kind() {
                gix::objs::tree::EntryKind::Tree => trees.push(entry.oid.to_owned()),
                gix::objs::tree::EntryKind::Commit => {}
                _ => match destination.find_object(entry.oid) {
                    Ok(_) => {}
                    // A partial (promisor) destination legitimately lacks leaf
                    // blobs, and validate_fetched_anchor has already bound this
                    // OID into the filtered host anchor tree, so the promise is
                    // to exactly the content host history names. An ordinary
                    // repository keeps failing closed, as do missing trees and
                    // commits everywhere.
                    Err(gix::object::find::existing::Error::NotFound { .. })
                        if promised_leaves_allowed => {}
                    Err(err) => {
                        return Err(format!(
                            "anchor tree references missing blob {}: {err}",
                            entry.oid
                        ));
                    }
                },
            }
        }
    }
    Ok(())
}

fn record_shallow_boundary(
    destination: &gix::Repository,
    anchor: gix::ObjectId,
) -> Result<(), String> {
    let shallow_file = destination.shallow_file();
    let mut commits = BTreeSet::new();
    match fs::read_to_string(&shallow_file) {
        Ok(body) => commits.extend(body.lines().map(str::to_owned)),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => {}
        Err(err) => return Err(format!("could not read {}: {err}", shallow_file.display())),
    }
    commits.insert(anchor.to_string());
    let parent = shallow_file
        .parent()
        .ok_or_else(|| format!("{} has no parent directory", shallow_file.display()))?;
    let mut temporary = tempfile::NamedTempFile::new_in(parent)
        .map_err(|err| format!("could not create a shallow file: {err}"))?;
    for commit in commits {
        writeln!(temporary, "{commit}")
            .map_err(|err| format!("could not write the shallow boundary: {err}"))?;
    }
    temporary.persist(&shallow_file).map_err(|err| {
        format!(
            "could not install {}: {}",
            shallow_file.display(),
            err.error
        )
    })?;
    Ok(())
}

impl Git {
    fn command(&self) -> Command {
        let mut command = Command::new(&self.executable);
        // Hide console window on Windows (https://stackoverflow.com/a/60958956)
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt as _;
            const CREATE_NO_WINDOW: u32 = 0x08000000;
            command.creation_flags(CREATE_NO_WINDOW);
        }
        command
            // Same reason as in jj_lib::git_subprocess: nothing here talks to
            // the macOS fsmonitor daemon, and spawning it causes trouble.
            .args(["-c", "core.fsmonitor=false"])
            .arg("--git-dir")
            .arg(&self.git_dir)
            // Disable translation so the output can be parsed. Not LC_ALL=C,
            // which would change the encoding.
            .env_remove("LC_ALL")
            .env_remove("LANGUAGE")
            .env("LC_MESSAGES", "C")
            .stdin(Stdio::null());
        command
    }

    /// Runs a git command, returning its stdout or everything it said.
    fn run(&self, mut command: Command) -> Result<String, String> {
        let output = command
            .output()
            .map_err(|err| format!("could not run {}: {err}", self.executable.display()))?;
        if output.status.success() {
            Ok(String::from_utf8_lossy(&output.stdout).into_owned())
        } else {
            let mut message = String::from_utf8_lossy(&output.stderr)
                .trim_end()
                .to_owned();
            if message.is_empty() {
                message = format!("git exited with {}", output.status);
            }
            Err(message)
        }
    }

    /// Counts commits reachable from `revision` after `ancestor`.
    ///
    /// Git reads its commit graph for this query. Walking the same graph
    /// through object lookups made an 88065-commit irrelevant merge parent
    /// cost more than 120 seconds while Git answers the count directly.
    fn history_count_after(
        &self,
        revision: &gix::ObjectId,
        ancestor: &gix::ObjectId,
    ) -> Result<usize, String> {
        let mut count = self.command();
        count
            .args(["rev-list", "--count"])
            .arg(revision.to_string())
            .arg(format!("^{ancestor}"));
        let output = self.run(count)?;
        output.trim().parse().map_err(|err| {
            format!(
                "Git returned an invalid history count {:?}: {err}",
                output.trim()
            )
        })
    }

    /// Returns every commit reachable from `revision` using Git's commit graph.
    fn reachable_commits(
        &self,
        revision: &gix::ObjectId,
    ) -> Result<HashSet<gix::ObjectId>, String> {
        let mut command = self.command();
        command.arg("rev-list").arg(revision.to_string());
        self.run(command)?
            .lines()
            .map(|id| {
                gix::ObjectId::from_hex(id.as_bytes())
                    .map_err(|err| format!("Git returned an invalid reachable commit {id}: {err}"))
            })
            .collect()
    }

    /// Brings the published repository's branch into this store, under a ref
    /// only this command writes.
    ///
    /// Forced, because the tracking ref is a mirror of what upstream says and
    /// an upstream that rewrote history should still be observable here. What
    /// that rewrite means for this repository is decided afterwards, in the
    /// view's own hashes, where it can be reported rather than merged.
    ///
    /// The returned store was opened after the fetch. gix fixes its pack-index
    /// slot count at open time, so the handle from before a fetch cannot always
    /// read a pack that fetch retained.
    fn fetch(&self, remote: &str, branch: &str, into: &str) -> Result<gix::Repository, String> {
        let mut command = self.command();
        command
            .args(["fetch", "--no-tags", "--", remote])
            .arg(format!("+refs/heads/{branch}:{into}"));
        self.run(command)?;
        gix::open(&self.git_dir)
            .map_err(|err| format!("could not reopen the Git store after fetching: {err}"))
    }

    /// Brings one adopted tip and its snapshot into this store, under `into`.
    ///
    /// Depth 1, with blobs. The fetched commit is about to become a view's
    /// anchor and its tree the content of a host change, so unlike the anchor
    /// fetch the blobs must arrive; unlike [`Self::fetch`], the adopted
    /// history's older ancestry stays out, recorded as a shallow boundary in
    /// this store exactly as an installed anchor records one. `selector` is a
    /// fully qualified branch ref or a full object id, either of which git
    /// accepts as a refspec source.
    fn fetch_adopt_tip(
        &self,
        remote: &str,
        selector: &str,
        into: &str,
    ) -> Result<gix::Repository, String> {
        let mut command = self.command();
        command
            .args(["fetch", "--no-tags", "--depth=1", "--", remote])
            .arg(format!("+{selector}:{into}"));
        self.run(command)?;
        gix::open(&self.git_dir)
            .map_err(|err| format!("could not reopen the Git store after fetching: {err}"))
    }

    /// Fetches a bounded history into a temporary shallow repository.
    ///
    /// The temporary boundary keeps the commit's older ancestry out of the jj
    /// store. Blobs are omitted because anchor validation compares tree ids.
    fn fetch_shallow(
        &self,
        remote: &str,
        selector: &str,
        depth: usize,
        anchor: gix::ObjectId,
    ) -> Result<ShallowFetch, String> {
        let dir = tempfile::tempdir().map_err(|err| format!("could not make a temp dir: {err}"))?;
        let output = Command::new(&self.executable)
            .args(["init", "--bare", "--quiet"])
            .arg(dir.path())
            .output()
            .map_err(|err| format!("could not run {}: {err}", self.executable.display()))?;
        if !output.status.success() {
            return Err(String::from_utf8_lossy(&output.stderr)
                .trim_end()
                .to_owned());
        }
        let temporary = Self {
            executable: self.executable.clone(),
            git_dir: dir.path().to_owned(),
        };
        let mut add_remote = temporary.command();
        add_remote.args(["remote", "add", "anchor-source", remote]);
        temporary.run(add_remote)?;
        let mut fetch = temporary.command();
        fetch
            .args([
                "fetch",
                "--no-tags",
                &format!("--depth={depth}"),
                "--filter=blob:none",
                "--",
                "anchor-source",
            ])
            .arg(selector);
        temporary.run(fetch)?;

        let mut history = temporary.command();
        history.args(["rev-list", "--parents", "--reverse", "FETCH_HEAD"]);
        validate_shallow_history(&temporary.run(history)?, depth, anchor)?;

        let mut resolve_tip = temporary.command();
        resolve_tip.args(["rev-parse", "--verify", "FETCH_HEAD^{commit}"]);
        let tip_text = temporary.run(resolve_tip)?;
        let tip = gix::ObjectId::from_hex(tip_text.trim().as_bytes())
            .map_err(|err| format!("Git returned an invalid fetched tip: {err}"))?;

        let mut read = temporary.command();
        read.args(["cat-file", "commit", &anchor.to_string()]);
        let output = read
            .output()
            .map_err(|err| format!("could not run {}: {err}", self.executable.display()))?;
        if !output.status.success() {
            return Err(String::from_utf8_lossy(&output.stderr)
                .trim_end()
                .to_owned());
        }
        let anchor_commit = output.stdout;

        let mut reference = temporary.command();
        reference.args(["update-ref", "refs/jj/anchor-fetch", &anchor.to_string()]);
        temporary.run(reference)?;

        Ok(ShallowFetch {
            directory: dir,
            tip,
            anchor,
            anchor_commit,
        })
    }

    /// Installs a validated anchor as a shallow boundary in the jj Git store.
    fn install_shallow_anchor(&self, prepared: &ShallowFetch) -> Result<(), String> {
        let source = gix::open(prepared.directory.path())
            .map_err(|err| format!("could not open the validated anchor repository: {err}"))?;
        let destination = gix::open(&self.git_dir)
            .map_err(|err| format!("could not open the jj Git store: {err}"))?;
        install_anchor_objects(&source, &destination, prepared)?;
        record_shallow_boundary(&destination, prepared.anchor)
    }

    /// Paths each of `commits` touches, in the view's own coordinates.
    ///
    /// One invocation rather than one per commit, because the interesting case
    /// is an upstream that advanced by tens of commits and the report should
    /// not cost a subprocess each. The commits are named rather than given as
    /// a range: the set being lifted is not a range, since a commit already
    /// integrated and dropped by the view sits inside `derived..upstream`
    /// without being lifted, and a range would count it.
    fn touched_paths(
        &self,
        commits: &[gix::ObjectId],
    ) -> Result<Vec<(String, Vec<String>)>, String> {
        let Some(first) = commits.first() else {
            return Ok(Vec::new());
        };
        let hash_len = first.kind().len_in_hex();
        let mut command = self.command();
        command.args([
            "log",
            "--no-walk",
            "--name-only",
            "--no-renames",
            "--format=%H",
        ]);
        command.args(commits.iter().map(gix::ObjectId::to_string));
        let stdout = self.run(command)?;
        let mut out: Vec<(String, Vec<String>)> = Vec::new();
        for line in stdout.lines() {
            let line = line.trim_end();
            if line.is_empty() {
                continue;
            }
            // A commit header is bare hex of the repository's hash length;
            // anything else on its own line is a path.
            if line.len() == hash_len && line.bytes().all(|b| b.is_ascii_hexdigit()) {
                out.push((line.to_owned(), Vec::new()));
            } else if let Some((_, paths)) = out.last_mut() {
                paths.push(line.to_owned());
            }
        }
        Ok(out)
    }

    /// Paths that differ between two commits, restricted to a prefix.
    fn changed_under(
        &self,
        from: gix::ObjectId,
        to: gix::ObjectId,
        prefix: &str,
    ) -> Result<Vec<String>, String> {
        let mut command = self.command();
        command
            .args(["diff", "--name-only", "--no-renames"])
            .arg(from.to_string())
            .arg(to.to_string())
            .arg("--")
            .arg(prefix);
        Ok(self
            .run(command)?
            .lines()
            .map(str::trim_end)
            .filter(|line| !line.is_empty())
            .map(str::to_owned)
            .collect())
    }

    /// Rejects a branch name git itself would reject, before anything is sent.
    fn check_ref_format(&self, branch: &str) -> Result<(), CommandError> {
        let mut command = self.command();
        command.args(["check-ref-format", &format!("refs/heads/{branch}")]);
        self.run(command)
            .map(|_| ())
            .map_err(|_| user_error(format!("Not a valid branch name: {branch}")))
    }

    /// What the remote's branch points at now, or `None` if it has no such
    /// branch.
    ///
    /// The distinction matters and is not visible in the output alone: an empty
    /// answer and a failed connection both print nothing, so the exit status is
    /// what separates them.
    fn remote_branch(&self, remote: &str, branch: &str) -> Result<Option<gix::ObjectId>, String> {
        let mut command = self.command();
        command
            .args(["ls-remote", "--", remote])
            .arg(format!("refs/heads/{branch}"));
        let stdout = self.run(command)?;
        let Some(line) = stdout.lines().next() else {
            return Ok(None);
        };
        let hex = line.split_whitespace().next().unwrap_or_default();
        gix::ObjectId::from_hex(hex.as_bytes())
            .map(Some)
            .map_err(|err| format!("could not read what {remote} said about {branch}: {err}"))
    }

    /// Writes `tip` to the remote's branch, keeping what it replaced.
    ///
    /// `observed` is what `ls-remote` just said the branch was, and becomes a
    /// lease: an amended revision re-derives to a commit that is not a
    /// descendant of the last one, so this has to be able to replace a branch
    /// rather than only extend it, and the lease is what keeps that from
    /// silently discarding somebody else's push in between.
    ///
    /// `pin` names a ref the branch's current tip is written to, for the case
    /// where the push drops it from the branch's history entirely. It travels
    /// in this push rather than a preceding one because `--atomic` is what
    /// makes "the old tip is still named" and "the branch moved" one fact:
    /// a pin that landed while the replacement failed leaves a ref nobody
    /// asked for, and a replacement that landed while the pin failed leaves
    /// every `flake.lock` pinning the old revision unresolvable.
    fn push(
        &self,
        remote: &str,
        tip: gix::ObjectId,
        branch: &str,
        observed: Option<gix::ObjectId>,
        pin: Option<(&str, gix::ObjectId)>,
    ) -> Result<(), String> {
        let mut command = self.command();
        // jj does not run commit hooks, so neither does this.
        command.args(["push", "--atomic", "--no-verify"]);
        if let Some(observed) = observed {
            command.arg(format!("--force-with-lease=refs/heads/{branch}:{observed}"));
        }
        command.args(["--", remote]);
        // The pin is listed first, so the command and the reflog it leaves both
        // read in the order the guarantee is stated: the old tip is named, then
        // the branch that named it moves.
        if let Some((name, replaced)) = pin {
            command.arg(format!("{replaced}:{name}"));
        }
        command.arg(format!("{tip}:refs/heads/{branch}"));
        self.run(command).map(|_| ())
    }
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use super::*;

    /// An anchor fixture whose tree names a leaf blob that exists nowhere: the
    /// shape a blob:none lane presents for deliberately absent view blobs.
    fn anchor_with_promised_leaf(source: &gix::Repository) -> ShallowFetch {
        let missing_blob = gix::ObjectId::from_hex(b"7bd71de35c9e4b6a8f2d91c3e5a7b90412345678")
            .expect("valid blob id");
        let tree = gix::objs::Tree {
            entries: vec![gix::objs::tree::Entry {
                mode: gix::objs::tree::EntryKind::Blob.into(),
                filename: "missing-view-blob".into(),
                oid: missing_blob,
            }],
        };
        let tree_id = source
            .write_object(&tree)
            .expect("write anchor tree")
            .detach();
        anchor_for_tree(source, tree_id)
    }

    fn anchor_for_tree(source: &gix::Repository, tree_id: gix::ObjectId) -> ShallowFetch {
        let anchor_commit = format!(
            "tree {tree_id}\nauthor t <t@t.invalid> 0 +0000\ncommitter t <t@t.invalid> 0 \
             +0000\n\nanchor fixture\n"
        )
        .into_bytes();
        let anchor = source
            .objects
            .write_buf(gix::objs::Kind::Commit, &anchor_commit)
            .expect("write anchor commit");
        ShallowFetch {
            directory: tempfile::tempdir().expect("fixture directory"),
            tip: anchor,
            anchor,
            anchor_commit,
        }
    }

    fn promisor_destination(root: &Path) -> gix::Repository {
        let path = root.join("promisor-destination");
        gix::init_bare(&path).expect("init promisor destination");
        let config = path.join("config");
        let mut body = fs::read_to_string(&config).expect("read destination config");
        body.push_str("[remote \"origin\"]\n\tpromisor = true\n\tpartialclonefilter = blob:none\n");
        fs::write(&config, body).expect("mark destination as promisor");
        gix::open(&path).expect("open promisor destination")
    }

    #[test]
    fn a_promisor_destination_accepts_promised_missing_leaf_blobs() {
        let scratch = tempfile::tempdir().expect("scratch");
        let source = gix::init_bare(scratch.path().join("source")).expect("init source");
        let prepared = anchor_with_promised_leaf(&source);
        let destination = promisor_destination(scratch.path());

        install_anchor_objects(&source, &destination, &prepared)
            .expect("a promisor destination treats the absent leaf blob as promised");
        destination
            .find_object(prepared.anchor)
            .expect("anchor commit is installed exactly");
        let commit =
            gix::objs::CommitRef::from_bytes(&prepared.anchor_commit, source.object_hash())
                .expect("decode anchor commit");
        destination
            .find_object(commit.tree())
            .expect("anchor tree is installed exactly");
    }

    #[test]
    fn an_ordinary_destination_still_fails_closed_on_a_missing_leaf_blob() {
        let scratch = tempfile::tempdir().expect("scratch");
        let source = gix::init_bare(scratch.path().join("source")).expect("init source");
        let prepared = anchor_with_promised_leaf(&source);
        let destination =
            gix::init_bare(scratch.path().join("ordinary-destination")).expect("init destination");

        let error = install_anchor_objects(&source, &destination, &prepared)
            .expect_err("an ordinary repository must reject the missing leaf blob");
        assert!(error.contains("missing blob"), "{error}");
    }

    #[test]
    fn a_promisor_destination_still_requires_trees() {
        let scratch = tempfile::tempdir().expect("scratch");
        let source = gix::init_bare(scratch.path().join("source")).expect("init source");
        let absent_tree = gix::ObjectId::from_hex(b"9a1f2b3c4d5e6f708192a3b4c5d6e7f801234567")
            .expect("valid tree id");
        let prepared = anchor_for_tree(&source, absent_tree);
        let destination = promisor_destination(scratch.path());

        let error = install_anchor_objects(&source, &destination, &prepared)
            .expect_err("a missing tree must fail even in a promisor repository");
        assert!(error.contains("could not read anchor tree"), "{error}");
    }
}
