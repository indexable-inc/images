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
use clap_complete::ArgValueCompleter;
use jj_lib::object_id::ObjectId as _;
use jj_views::Cache;
use tracing::instrument;

use super::ViewConfig;
use super::get_views_config;
use super::lift_error;
use super::open_git_repo;
use super::select_views;
use crate::cli_util::CommandHelper;
use crate::cli_util::RevisionArg;
use crate::command_error::CommandError;
use crate::command_error::user_error;
use crate::command_error::user_error_with_message;
use crate::complete;
use crate::ui::Ui;

/// List commits a view would publish with a message describing work outside it
///
/// A view sends a filtered file list and an unfiltered commit message. A commit
/// that changed both the view's path and the rest of this repository therefore
/// publishes a message written about work the view does not show, which is how
/// a note meant for this repository alone reaches a repository other people
/// read.
///
/// This reads objects and touches no network, so it is cheap enough to run as a
/// gate before `jj views push`. It exits non-zero when it finds anything, and
/// says nothing when it does not.
///
/// Only commits with one parent are reported. A root would be compared against
/// nothing and so would name the whole repository, and a merge's tree differs
/// from every parent outside the prefix whenever both sides changed anything out
/// there, so it would name every integration merge. Neither answer is about a
/// message, and the content both carry was written in commits this listing
/// already reaches.
#[derive(clap::Args, Clone, Debug)]
pub struct ViewsCheckArgs {
    /// View to check, by its key in the `views` config table (can be repeated)
    ///
    /// Defaults to every configured view.
    #[arg(value_name = "VIEW")]
    #[arg(add = ArgValueCandidates::new(complete::views))]
    views: Vec<String>,

    /// Revision whose history to check
    #[arg(long, short, default_value = "@", value_name = "REVSET")]
    #[arg(add = ArgValueCompleter::new(complete::revset_expression_all))]
    revision: RevisionArg,

    /// Check every reachable commit, not only those after each view's anchor
    ///
    /// An anchor marks history the view has already published, so the default
    /// stops there. Everything older has either been sent or been decided not
    /// to be, and re-reporting it cannot change either.
    #[arg(long)]
    all: bool,
}

#[instrument(skip_all)]
pub async fn cmd_views_check(
    ui: &mut Ui,
    command: &CommandHelper,
    args: &ViewsCheckArgs,
) -> Result<(), CommandError> {
    let workspace_command = command.workspace_helper(ui).await?;
    let configured = get_views_config(&workspace_command).await?;
    let selected = select_views(&configured, &args.views)?;

    let commit = workspace_command
        .resolve_single_rev(ui, &args.revision)
        .await?;
    // A gate that cannot read the objects refuses; it does not report a clean
    // scan. `check` exists to catch a commit whose message describes work its
    // view does not show, and a repository whose history it cannot walk is a
    // question it has not answered rather than one that came back empty.
    let repo = open_git_repo(&workspace_command, "check")?;
    let source = gix::ObjectId::try_from(commit.id().as_bytes())
        .map_err(|err| user_error_with_message("Commit is not a Git object", err))?;

    let mut cache = Cache::new();
    let mut found = 0;
    for view in &selected {
        let mixed = scan(&repo, view, &source, args.all, &mut cache)?;
        found += mixed.len();
        let mut out = ui.stdout();
        for candidate in &mixed {
            let paths = candidate
                .outside
                .iter()
                .map(bstr::BString::to_string)
                .collect::<Vec<_>>()
                .join(" ");
            writeln!(
                out,
                "{} {} {}",
                view.name,
                candidate.commit.to_hex_with_len(12),
                paths
            )?;
        }
    }
    if found == 0 {
        return Ok(());
    }
    Err(user_error(format!(
        "{found} commit{} would publish a message describing work outside its view",
        if found == 1 { "" } else { "s" }
    ))
    .hinted(
        "Split each one with `jj split` so the view's paths travel with a message about them, or \
         push with --allow-mixed to accept it.",
    ))
}

fn scan(
    repo: &gix::Repository,
    view: &ViewConfig,
    source: &gix::ObjectId,
    all: bool,
    cache: &mut Cache,
) -> Result<Vec<jj_views::MixedCommit>, CommandError> {
    let filter = view.filter()?;
    let anchor = (!all)
        .then(|| view.anchor.as_ref().map(|anchor| anchor.source))
        .flatten();
    jj_views::mixed_commits(
        repo,
        source,
        anchor.as_deref(),
        &filter,
        &super::exempt_paths(),
        cache,
    )
    .map_err(|err| lift_error(view, err))
}
