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

use std::fmt;
use std::io::Write as _;

use clap_complete::ArgValueCandidates;
use jj_views::Cache;
use tracing::instrument;

use super::ANCHOR_REF_NAMESPACE;
use super::ANCHOR_REVISION_REF_NAMESPACE;
use super::ANCHOR_SOURCE_REF_NAMESPACE;
use super::ShallowFetch;
use super::anchor;
use super::commit_tree;
use super::commits;
use super::get_views_config;
use super::lift_error;
use super::open_store;
use super::select_views;
use crate::cli_util::CommandHelper;
use crate::command_error::CommandError;
use crate::command_error::config_error;
use crate::command_error::user_error;
use crate::command_error::user_error_with_message;
use crate::complete;
use crate::ui::Ui;

/// Fetch and validate one manifest anchor without its older history
#[derive(clap::Args, Clone, Debug)]
pub struct ViewsAnchorArgs {
    /// View whose manifest anchor should be installed (can be repeated)
    ///
    /// Defaults to every configured view.
    #[arg(value_name = "VIEW")]
    #[arg(add = ArgValueCandidates::new(complete::views))]
    views: Vec<String>,

    /// Bookmark whose history must contain the source anchor
    #[arg(long, short = 'b', default_value = "main", value_name = "NAME")]
    #[arg(add = ArgValueCandidates::new(complete::local_bookmarks))]
    bookmark: String,

    /// Verify a published branch that is behind this repository's derivation
    ///
    /// Behind means the published tip is an ancestor of the derived tip:
    /// nothing diverged, and `jj views push <view> --branch <its branch>
    /// --allow-default-branch` fast-forwards it. A gate that runs before that
    /// push has to be able to answer "valid, pending a fast-forward" without
    /// failing, so this reports the state in the output instead of refusing.
    /// Drift and divergence still fail: no fast-forward can repair those.
    #[arg(long)]
    allow_behind: bool,

    /// Emit one stable JSON object containing every selected anchor
    #[arg(long)]
    json: bool,
}

#[derive(serde::Serialize)]
struct AnchorOutput {
    views: Vec<AnchorStatus>,
}

#[derive(serde::Serialize)]
struct AnchorStatus {
    name: String,
    source: String,
    view: String,
    fetched_commits: usize,
    tree_matches: bool,
    /// Whether the published branch is behind the derivation and waiting on a
    /// fast-forward push. Only `--allow-behind` reaches this state; without
    /// the flag the same situation is an error, so a `false` here means the
    /// published history verified outright.
    published_behind: bool,
    endpoint: AnchorEndpointKind,
    attempts: Vec<AnchorEndpointAttempt>,
}

#[derive(Clone, Copy, Debug, serde::Serialize)]
#[serde(rename_all = "snake_case")]
enum AnchorEndpointKind {
    Local,
    Upstream,
    Published,
}

impl fmt::Display for AnchorEndpointKind {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Local => formatter.write_str("local object store"),
            Self::Upstream => formatter.write_str("read-only upstream"),
            Self::Published => formatter.write_str("published branch"),
        }
    }
}

#[derive(Clone, Debug, serde::Serialize)]
struct AnchorEndpointAttempt {
    endpoint: AnchorEndpointKind,
    remote: String,
    selector: String,
    error: String,
}

#[derive(Debug)]
struct AnchorFetchError {
    anchor: gix::ObjectId,
    attempts: Vec<AnchorEndpointAttempt>,
}

impl fmt::Display for AnchorFetchError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "anchor {} was not available", self.anchor)?;
        for attempt in &self.attempts {
            write!(
                formatter,
                "; {} {} at {} failed: {}",
                attempt.endpoint, attempt.selector, attempt.remote, attempt.error
            )?;
        }
        Ok(())
    }
}

impl std::error::Error for AnchorFetchError {}

#[derive(Debug, thiserror::Error)]
enum PublishedHistoryError {
    #[error("host commit {host_commit} produced no published view commit")]
    ElidedHostCommit { host_commit: gix::ObjectId },
    /// The two tips carry the same tree. Nothing under the prefix differs; the
    /// published branch simply reached that content through commit objects the
    /// host derivation did not produce, which is what makes this recoverable
    /// where a content difference is not.
    #[error(
        "published tip {actual_tip} and derived host tip {expected_tip} carry the same tree \
         {tree}, so the published branch holds hash-drifted copies of the commits this repository \
         derives"
    )]
    Drift {
        expected_tip: gix::ObjectId,
        actual_tip: gix::ObjectId,
        tree: gix::ObjectId,
    },
    /// The published tip is an ancestor of the derived tip. Nothing drifted
    /// and nothing diverged; the published branch has not been pushed yet.
    #[error(
        "published tip {actual_tip} is an ancestor of derived host tip {expected_tip}: the \
         published branch is behind this repository's derivation"
    )]
    Behind {
        expected_tip: gix::ObjectId,
        actual_tip: gix::ObjectId,
    },
    #[error(
        "published tip is {actual_tip} with tree {actual_tree}; derived host tip is \
         {expected_tip} with tree {expected_tree}"
    )]
    Mismatch {
        expected_tip: gix::ObjectId,
        actual_tip: gix::ObjectId,
        expected_tree: gix::ObjectId,
        actual_tree: gix::ObjectId,
    },
}

/// What verifying a published branch's history concluded, when it concluded
/// rather than failed.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum PublishedHistory {
    /// The published branch carries what this repository derives.
    Current,
    /// The published tip is an ancestor of the derived tip, so a plain
    /// fast-forward push is what reconciles it. Reached only under
    /// `--allow-behind`; without the flag the same state is
    /// [`PublishedHistoryError::Behind`].
    Behind,
}

struct AnchorEndpoint<'a> {
    kind: AnchorEndpointKind,
    remote: &'a str,
    selector: String,
    depth: usize,
}

#[instrument(skip_all)]
pub async fn cmd_views_anchor(
    ui: &mut Ui,
    command: &CommandHelper,
    args: &ViewsAnchorArgs,
) -> Result<(), CommandError> {
    let workspace_command = command.workspace_helper(ui).await?;
    let configured = get_views_config(&workspace_command).await?;
    let selected = select_views(&configured, &args.views)?;
    let (git, repo) = open_store(&workspace_command, "anchor")?;
    let (_, revision) = anchor(&workspace_command, &args.bookmark, "anchor")?;
    let reachable = git.reachable_commits(&revision).map_err(|err| {
        user_error(format!(
            "Could not read commits reachable from {revision}: {err}"
        ))
    })?;
    let mut statuses = Vec::with_capacity(selected.len());
    let mut cache = Cache::new();
    for view in selected {
        let manifest_anchor = view.anchor.ok_or_else(|| {
            config_error(format!(
                "The {} view has no anchor in .jj-views.toml",
                view.name
            ))
        })?;
        if !reachable.contains(&manifest_anchor.source) {
            return Err(user_error(format!(
                "The {} view anchor source {} is not an ancestor of {revision}",
                view.name, manifest_anchor.source
            )));
        }
        let filter = view.filter()?;
        if view.root_anchor {
            cache
                .create_root_anchor_after_ancestry_check(&repo, &filter, manifest_anchor)
                .map_err(|err| lift_error(view, err))?;
            write_anchor_refs(&repo, &view.name, manifest_anchor, revision)?;
            statuses.push(AnchorStatus {
                name: view.name.clone(),
                source: manifest_anchor.source.to_string(),
                view: manifest_anchor.view.to_string(),
                fetched_commits: 0,
                tree_matches: true,
                published_behind: false,
                endpoint: AnchorEndpointKind::Local,
                attempts: Vec::new(),
            });
            continue;
        }
        let anchor_is_present = match repo.find_object(manifest_anchor.view) {
            Ok(_) => true,
            Err(gix::object::find::existing::Error::NotFound { .. }) => false,
            Err(err) => {
                return Err(user_error_with_message(
                    format!("Could not read the {} view anchor", view.name),
                    err,
                ));
            }
        };
        let (fetched_commits, endpoint, attempts, published_behind) = if anchor_is_present {
            cache
                .seed_anchor_after_ancestry_check(&repo, &filter, manifest_anchor)
                .map_err(|err| lift_error(view, err))?;
            write_anchor_refs(&repo, &view.name, manifest_anchor, revision)?;
            (0, AnchorEndpointKind::Local, Vec::new(), false)
        } else {
            let host_commit_count = git
                .history_count_after(&revision, &manifest_anchor.source)
                .map_err(|err| {
                    user_error(format!(
                        "Could not count host commits for the {} view: {err}",
                        view.name
                    ))
                })?;
            let mut endpoints = Vec::with_capacity(2);
            endpoints.push(AnchorEndpoint {
                kind: AnchorEndpointKind::Published,
                remote: &view.remote,
                selector: format!("refs/heads/{}", view.branch),
                depth: host_commit_count.checked_add(1).ok_or_else(|| {
                    user_error(format!("The {} view's fetch depth overflowed", view.name))
                })?,
            });
            if let Some(upstream) = &view.upstream {
                endpoints.push(AnchorEndpoint {
                    kind: AnchorEndpointKind::Upstream,
                    remote: &upstream.remote,
                    selector: manifest_anchor.view.to_string(),
                    depth: 1,
                });
            }

            let mut attempts = Vec::new();
            let mut fetched = None;
            for endpoint in endpoints {
                match git.fetch_shallow(
                    endpoint.remote,
                    &endpoint.selector,
                    endpoint.depth,
                    manifest_anchor.view,
                ) {
                    Ok(prepared) => {
                        fetched = Some((endpoint.kind, prepared));
                        break;
                    }
                    Err(error) => attempts.push(AnchorEndpointAttempt {
                        endpoint: endpoint.kind,
                        remote: endpoint.remote.to_owned(),
                        selector: endpoint.selector,
                        error,
                    }),
                }
            }
            let Some((endpoint, prepared)) = fetched else {
                return Err(user_error_with_message(
                    format!(
                        "Could not fetch the {} view anchor {}",
                        view.name, manifest_anchor.view
                    ),
                    AnchorFetchError {
                        anchor: manifest_anchor.view,
                        attempts,
                    },
                ));
            };
            cache
                .validate_fetched_anchor_after_ancestry_check(
                    &repo,
                    &filter,
                    manifest_anchor,
                    &prepared.anchor_commit,
                )
                .map_err(|err| lift_error(view, err))?;
            let published_behind = match endpoint {
                AnchorEndpointKind::Published => {
                    validate_published_history(
                        view,
                        &repo,
                        &filter,
                        &revision,
                        &prepared,
                        args.allow_behind,
                        &mut cache,
                    )? == PublishedHistory::Behind
                }
                AnchorEndpointKind::Local | AnchorEndpointKind::Upstream => false,
            };
            git.install_shallow_anchor(&prepared).map_err(|err| {
                user_error(format!(
                    "Could not install the {} view's shallow anchor: {err}",
                    view.name
                ))
            })?;
            write_anchor_refs(&repo, &view.name, manifest_anchor, revision)?;
            (1, endpoint, attempts, published_behind)
        };
        statuses.push(AnchorStatus {
            name: view.name.clone(),
            source: manifest_anchor.source.to_string(),
            view: manifest_anchor.view.to_string(),
            fetched_commits,
            tree_matches: true,
            published_behind,
            endpoint,
            attempts,
        });
    }
    if args.json {
        let json = serde_json::to_string(&AnchorOutput { views: statuses })
            .map_err(|err| user_error_with_message("Could not encode view anchors", err))?;
        writeln!(ui.stdout(), "{json}")?;
    } else {
        let mut out = ui.status();
        for status in statuses {
            for attempt in &status.attempts {
                writeln!(
                    out,
                    "{}: {} {} at {} failed: {}",
                    status.name, attempt.endpoint, attempt.selector, attempt.remote, attempt.error
                )?;
            }
            writeln!(
                out,
                "{}: anchor {} -> {} is valid from {}; fetched {} and its tree matches.",
                status.name,
                status.source,
                status.view,
                status.endpoint,
                commits(status.fetched_commits)
            )?;
            if status.published_behind {
                writeln!(
                    out,
                    "{}: the published branch is behind this repository's derivation; `jj views \
                     push {} --branch <its branch> --allow-default-branch` fast-forwards it.",
                    status.name, status.name
                )?;
            }
        }
    }
    Ok(())
}

fn validate_published_history(
    view: &super::ViewConfig,
    repo: &gix::Repository,
    filter: &jj_views::Filter,
    revision: &gix::ObjectId,
    prepared: &ShallowFetch,
    allow_behind: bool,
    cache: &mut Cache,
) -> Result<PublishedHistory, CommandError> {
    let expected_tip = jj_views::derive_tip(repo, revision, filter, cache)
        .map_err(|err| lift_error(view, err))?
        .ok_or_else(|| {
            user_error_with_message(
                format!(
                    "Could not verify the {} published anchor history",
                    view.name
                ),
                PublishedHistoryError::ElidedHostCommit {
                    host_commit: *revision,
                },
            )
        })?;
    // A Git commit id hashes its parent ids recursively, so equal tips prove
    // the full filtered parent graph without requiring that graph to be linear.
    if expected_tip != prepared.tip {
        // The trees and the derived lineage separate failures that look
        // identical from the tips alone, and they need different answers.
        // Equal trees with a published tip the derivation cannot reach are a
        // published branch that reached the content this repository derives
        // through commit objects it did not produce, which only a replacing
        // push can reconcile. Unequal trees are content this repository does
        // not produce at all, or a branch that is merely behind.
        let expected_tree = commit_tree(repo, expected_tip)?;
        let actual_tree = published_tree(view, prepared)?;
        // A published tip the derived lineage CONTAINS is a branch that is
        // merely behind -- the fix is a push, and telling someone to
        // fetch-and-integrate there sends them in a circle. Only a published
        // tip the derivation cannot reach holds foreign content.
        let lineage = match view.anchor {
            Some(anchor) => jj_views::verify::ancestry_after(repo, &expected_tip, &anchor.view),
            None => jj_views::verify::ancestry(repo, &expected_tip),
        }
        .map_err(|err| lift_error(view, err))?;
        let published_is_ancestor = lineage.contains(&prepared.tip);
        if published_is_ancestor && expected_tree == actual_tree {
            // Behind by commits that change nothing under the prefix. Every
            // other tool already treats this state as current: `jj views
            // fetch` compares content and reports the view up to date, and
            // `jj views push` finds no content beyond the published tip and
            // refuses to publish the empty commits. Failing here would demand
            // an action no command can perform, so it verifies. (ENG-12041)
            return Ok(PublishedHistory::Current);
        }
        if published_is_ancestor && allow_behind {
            // The trees differ here: the tree-identical case returned above.
            // The published branch is strictly behind by content the
            // derivation carries, which is exactly the state a plain
            // fast-forward push repairs, and the caller asked to have that
            // state reported rather than refused so the push can happen after
            // this gate instead of before it.
            return Ok(PublishedHistory::Behind);
        }
        let message = format!(
            "Could not verify the {} published anchor history",
            view.name
        );
        return Err(if expected_tree == actual_tree {
            user_error_with_message(
                message,
                PublishedHistoryError::Drift {
                    expected_tip,
                    actual_tip: prepared.tip,
                    tree: expected_tree,
                },
            )
            .hinted(format!(
                "`jj views push --branch {} --allow-default-branch --replace-drifted` replaces \
                 them with the commits this repository derives, keeping the tip they replace on a \
                 pin ref. `jj views fetch` cannot: it compares content, finds none missing, and \
                 reports the {} view up to date.",
                view.branch, view.name
            ))
        } else if published_is_ancestor {
            user_error_with_message(
                message,
                PublishedHistoryError::Behind {
                    expected_tip,
                    actual_tip: prepared.tip,
                },
            )
            .hinted(format!(
                "The published branch is an ancestor of what this repository derives: behind, not \
                 divergent. `jj views push {} --branch {} --allow-default-branch` fast-forwards \
                 it.",
                view.name, view.branch
            ))
        } else {
            user_error_with_message(
                message,
                PublishedHistoryError::Mismatch {
                    expected_tip,
                    actual_tip: prepared.tip,
                    expected_tree,
                    actual_tree,
                },
            )
            .hinted(format!(
                "{} has view content this repository's derivation does not produce. Run `jj views \
                 fetch {}` to bring those commits in here and integrate them, then re-run this. \
                 Do not replace the published branch: the difference is content, not hashes.",
                view.remote, view.name
            ))
        });
    }
    Ok(PublishedHistory::Current)
}

/// The tree of the tip that was fetched from the published branch.
///
/// Read from the temporary shallow repository the fetch landed in, because
/// nothing has been installed into this repository's store yet: validation runs
/// before the install so that a published history that fails it leaves no
/// objects behind.
fn published_tree(
    view: &super::ViewConfig,
    prepared: &ShallowFetch,
) -> Result<gix::ObjectId, CommandError> {
    let fetched = gix::open(prepared.directory.path()).map_err(|err| {
        user_error_with_message(
            format!("Could not read the fetched {} published history", view.name),
            err,
        )
    })?;
    commit_tree(&fetched, prepared.tip)
}

fn write_anchor_refs(
    repo: &gix::Repository,
    name: &str,
    anchor: jj_views::DeriveAnchor,
    revision: gix::ObjectId,
) -> Result<(), CommandError> {
    for (namespace, commit) in [
        (ANCHOR_REF_NAMESPACE, anchor.view),
        (ANCHOR_SOURCE_REF_NAMESPACE, anchor.source),
        (ANCHOR_REVISION_REF_NAMESPACE, revision),
    ] {
        repo.reference(
            format!("{namespace}{name}"),
            commit,
            gix::refs::transaction::PreviousValue::Any,
            "jj views anchor",
        )
        .map_err(|err| {
            user_error_with_message(format!("Could not record the {name} anchor"), err)
        })?;
    }
    Ok(())
}
