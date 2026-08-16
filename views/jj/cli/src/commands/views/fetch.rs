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

use std::io::Write as _;

use clap_complete::ArgValueCandidates;
use itertools::Itertools as _;
use jj_lib::backend::CommitId;
use jj_lib::op_store::RefTarget;
use jj_lib::repo::Repo as _;
use jj_views::Cache;
use tracing::instrument;

use super::Freshness;
use super::Position;
use super::ViewConfig;
use super::anchor;
use super::commits;
use super::diverged_report;
use super::elided_note;
use super::get_views_config;
use super::lift;
use super::open_store;
use super::overlap_with_working_copy;
use super::record;
use super::select_views;
use super::survey;
use super::validate_endpoints;
use super::working_copy_range;
use crate::cli_util::CommandHelper;
use crate::command_error::CommandError;
use crate::complete;
use crate::ui::Ui;

/// Fetch each configured view's repository and lift its new commits into this
/// one
///
/// The inverse of `jj views push`. It fetches the published repository, turns
/// the commits it has and this one does not into commits under the view's path
/// prefix, and moves a bookmark to the result.
///
/// Like `jj git fetch`, it only fast-forwards, and like `jj git fetch` a view
/// whose two sides have each grown commits the other does not have is a state
/// it reports rather than a failure. The published commits are brought in
/// either way -- for a diverged view they land beside this repository's history
/// instead of on top of it, and the bookmark is left where it was.
///
/// Integrating them is then `jj new <bookmark> <revision>`, or whichever jj
/// command suits. There is no integration verb here on purpose: jj has no
/// `merge` command and it does have `rebase`, so anything `jj views` added
/// would be a second spelling of something the user already has, in a surface
/// that should only be doing the translation between this repository and the
/// published one.
#[derive(clap::Args, Clone, Debug)]
pub struct ViewsFetchArgs {
    /// View to fetch, by its key in the `views` config table (can be repeated)
    ///
    /// Defaults to every configured view.
    #[arg(value_name = "VIEW")]
    #[arg(add = ArgValueCandidates::new(complete::views))]
    views: Vec<String>,

    /// Fetch one view from its read-only upstream endpoint
    #[arg(long, value_name = "VIEW", conflicts_with = "views")]
    #[arg(add = ArgValueCandidates::new(complete::views))]
    upstream: Option<String>,

    /// Bookmark the lifted commits are added to
    // `-b` names the local side here and the remote side in `jj git fetch`,
    // which is a clash worth being deliberate about. Which branch is fetched is
    // not a choice this command offers: `views.<name>.branch` fixes it, because
    // the published repository's own default branch is what a view tracks. The
    // only thing left to name is where the result lands, and that is a
    // bookmark, so it is spelled the way jj spells bookmarks everywhere else.
    #[arg(long, short = 'b', default_value = "main", value_name = "NAME")]
    #[arg(add = ArgValueCandidates::new(complete::local_bookmarks))]
    bookmark: String,

    /// Fetch and report what would be lifted, without changing anything
    #[arg(long)]
    dry_run: bool,
}

#[instrument(skip_all)]
pub async fn cmd_views_fetch(
    ui: &mut Ui,
    command: &CommandHelper,
    args: &ViewsFetchArgs,
) -> Result<(), CommandError> {
    let mut workspace_command = command.workspace_helper(ui).await?;
    let configured = get_views_config(&workspace_command).await?;
    let selected_names = args
        .upstream
        .as_ref()
        .map(std::slice::from_ref)
        .unwrap_or(&args.views);
    let selected = select_views(&configured, selected_names)?;
    let (git, mut repo) = open_store(&workspace_command, "fetch")?;
    let (bookmark, local) = anchor(&workspace_command, &args.bookmark, "fetch")?;
    let mut cache = Cache::new();
    validate_endpoints(&git, &selected)?;

    // Fetching is a read on this side, so every view is fetched and positioned
    // before anything here moves. A divergence in the third view is then
    // reported without the first two having already been lifted.
    let mut surveys = Vec::with_capacity(selected.len());
    for view in &selected {
        surveys.push(survey(
            &git,
            &mut repo,
            view,
            &local,
            args.upstream.is_some(),
            Freshness::Fetch,
            &mut cache,
        )?);
    }

    // What the working copy has changed relative to its parent, computed once.
    let edited = working_copy_range(&workspace_command);

    let mut out = ui.status();
    // (view, new head, commits lifted, what the incoming work collides with)
    let mut lifted_heads: Vec<(&ViewConfig, gix::ObjectId, usize, Option<String>)> = Vec::new();
    // Published lineages brought in beside this repository's history, for a
    // view that has diverged. They are indexed like the fast-forwarded ones,
    // because a commit `jj new` cannot name is a commit that did not arrive,
    // but the bookmark is left where it was.
    let mut beside: Vec<(&ViewConfig, gix::ObjectId)> = Vec::new();
    let mut onto = local;
    for survey in &surveys {
        let view = survey.view;
        match survey.position {
            Position::Current => {
                writeln!(out, "{}: already up to date.", view.name)?;
                if survey.elided > 0 {
                    writeln!(out, "  {}", elided_note(survey.elided))?;
                }
            }
            Position::LocalAhead => writeln!(
                out,
                "{}: this repository is ahead of {}; run `jj views push`.",
                view.name, survey.remote
            )?,
            Position::FastForward => {
                if args.dry_run {
                    writeln!(
                        out,
                        "{}: would lift {} up to {}",
                        view.name,
                        commits(survey.incoming.len()),
                        survey.upstream
                    )?;
                    continue;
                }
                let head = lift(&repo, survey, &onto, &mut cache)?;
                let overlap =
                    overlap_with_working_copy(&git, view, &survey.incoming, edited.as_ref());
                onto = head;
                lifted_heads.push((view, head, survey.incoming.len(), overlap));
            }
            Position::Diverged => {
                if args.dry_run {
                    writeln!(
                        out,
                        "{}: diverged from {}; would bring {} in beside {}",
                        view.name,
                        survey.remote,
                        commits(survey.incoming.len()),
                        args.bookmark
                    )?;
                    continue;
                }
                // Onto the bookmark rather than onto a previous view's result,
                // and it makes no difference: a diverged view's commits all
                // have counterparts here for their parents, so `onto` is never
                // consulted. Passing the anchor keeps that visible.
                beside.push((view, lift(&repo, survey, &local, &mut cache)?));
            }
        }
    }

    if args.dry_run || (lifted_heads.is_empty() && beside.is_empty()) {
        // Nothing here moved, so the surveys still describe the repository;
        // record them for `jj views prompt`. A dry run records too: it did
        // fetch, and the point of the record is what the last fetch learned.
        for survey in &surveys {
            record::write(
                workspace_command.repo_path(),
                &args.bookmark,
                &local,
                survey,
            )?;
        }
        return Ok(());
    }

    // The bookmark is moved through jj rather than by writing `refs/heads/`
    // and importing. Writing the Git ref looks equivalent and is not: in a
    // colocated workspace it moves HEAD, which resets the working copy onto the
    // fetched history and abandons the commit that was being worked on. Fetching
    // must not touch the working copy, exactly as `jj git fetch` does not.
    let mut tx = workspace_command.start_transaction();
    // The lifted commits are already objects in the Git store, but jj indexes
    // commits rather than reading the object database on demand, so a commit it
    // has not seen fails as a corrupt index whether a bookmark or a `jj new`
    // reaches for it. Adding the head walks and indexes its ancestors, which is
    // exactly the set just lifted.
    for (_, head) in &beside {
        let commit = tx
            .repo()
            .store()
            .get_commit(&CommitId::from_bytes(head.as_bytes()))?;
        tx.repo_mut().add_head(&commit).await?;
    }
    let mut bookmark_left = false;
    if !lifted_heads.is_empty() {
        let head = CommitId::from_bytes(onto.as_bytes());
        let commit = tx.repo().store().get_commit(&head)?;
        tx.repo_mut().add_head(&commit).await?;
        // Only fast-forward, exactly as the doc above promises. The lifted
        // head normally descends from the anchor because lifting substitutes
        // the anchor in as a counterpart parent; where it does not, the head
        // sits beside the local history, and moving the bookmark onto it would
        // silently discard every change the bookmark has that the lifted
        // ancestry does not. The commits are in and indexed either way, so
        // integrating them stays one `jj new` away.
        let anchor_id = CommitId::from_bytes(local.as_bytes());
        if tx.repo().index().is_ancestor(&anchor_id, &head).await? {
            tx.repo_mut()
                .set_local_bookmark_target(&bookmark, RefTarget::normal(head));
        } else {
            bookmark_left = true;
        }
    }
    let names = lifted_heads
        .iter()
        .map(|(view, ..)| &view.name)
        .chain(beside.iter().map(|(view, _)| &view.name))
        .join(", ");
    tx.finish(
        ui,
        if lifted_heads.is_empty() {
            format!("fetch views {names}")
        } else {
            format!("fetch views {names} into {}", args.bookmark)
        },
    )
    .await?;

    // Where each view stands now, recorded for `jj views prompt`. A fresh
    // survey rather than the pre-lift one, because the lift just changed the
    // answer; the cache the lift warmed makes asking again cheap.
    let anchor_after = if lifted_heads.is_empty() { local } else { onto };
    for view in &selected {
        let survey = survey(
            &git,
            &mut repo,
            view,
            &anchor_after,
            args.upstream.is_some(),
            Freshness::AsOfLastFetch,
            &mut cache,
        )?;
        record::write(
            workspace_command.repo_path(),
            &args.bookmark,
            &anchor_after,
            &survey,
        )?;
    }

    for (view, head, count, overlap) in &lifted_heads {
        writeln!(out, "{}: advanced {} to {head}", view.name, commits(*count))?;
        if let Some(overlap) = overlap {
            writeln!(out, "  {overlap}")?;
        }
    }
    if bookmark_left {
        writeln!(
            out,
            "The lifted history does not descend from {0}, so {0} was not moved.",
            args.bookmark
        )?;
        writeln!(
            out,
            "  Integrate it with `jj new {0} {1}`, then point {0} at the result.",
            args.bookmark, onto
        )?;
    }
    for (view, head) in &beside {
        let survey = surveys
            .iter()
            .find(|survey| std::ptr::eq(survey.view, *view))
            .expect("the view was surveyed");
        writeln!(
            out,
            "{}: diverged from {}; {} arrived and {} was not moved.",
            view.name,
            survey.remote,
            commits(survey.incoming.len()),
            args.bookmark
        )?;
        for line in diverged_report(survey, *head, &args.bookmark) {
            writeln!(out, "  {line}")?;
        }
    }
    Ok(())
}
