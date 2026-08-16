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
use jj_lib::backend::CommitId;
use jj_lib::object_id::ObjectId as _;
use jj_lib::repo::Repo as _;
use jj_views::Cache;
use tracing::instrument;

use super::Freshness;
use super::Position;
use super::Survey;
use super::ViewConfig;
use super::anchor;
use super::commits;
use super::elided_note;
use super::get_views_config;
use super::local_bookmark_commit;
use super::record;
use super::require_tracking_refs;
use super::select_views;
use super::survey;
use super::try_open_store;
use super::validate_endpoints;
use crate::cli_util::CommandHelper;
use crate::cli_util::WorkspaceCommandHelper;
use crate::command_error::CommandError;
use crate::command_error::user_error_with_message;
use crate::complete;
use crate::ui::Ui;

/// Say where each configured view stands against its published repository
///
/// Nothing in this repository moves: no commit is lifted, no bookmark is
/// written, and a diverged view is reported rather than refused, so one run
/// answers for every view instead of stopping at the first problem. It does
/// fetch, since the question is about a repository somewhere else, and that
/// updates the same tracking ref `jj views fetch` keeps and nothing besides.
/// `--no-fetch` answers from the last fetch and touches no network.
///
/// The number worth knowing about is the elided one. A commit that arrived and
/// that the view then drops -- an upstream merge that changed nothing under the
/// prefix, most often -- stays inside `git rev-list <view tip>..<upstream>`
/// forever, so raw ancestry reports a view as behind when nothing is missing
/// and no amount of fetching will change it. "3 commits behind" and "up to
/// date, 3 commits elided" are different situations and this is what tells
/// them apart.
#[derive(clap::Args, Clone, Debug)]
pub struct ViewsStatusArgs {
    /// View to report on, by its key in the `views` config table (can be
    /// repeated)
    ///
    /// Defaults to every configured view.
    #[arg(value_name = "VIEW")]
    #[arg(add = ArgValueCandidates::new(complete::views))]
    views: Vec<String>,

    /// Compare one view with its read-only upstream endpoint
    #[arg(long, value_name = "VIEW", conflicts_with = "views")]
    #[arg(add = ArgValueCandidates::new(complete::views))]
    upstream: Option<String>,

    /// Bookmark the views are derived from
    #[arg(long, short = 'b', default_value = "main", value_name = "NAME")]
    #[arg(add = ArgValueCandidates::new(complete::local_bookmarks))]
    bookmark: String,

    /// Report against the last fetch instead of asking the published
    /// repositories where they are now
    #[arg(long)]
    no_fetch: bool,

    /// Emit one stable JSON object containing every selected view
    #[arg(long)]
    json: bool,
}

#[derive(serde::Serialize)]
struct StatusOutput {
    /// Why no view carries counts, when none of them can.
    ///
    /// Skipped when the views were compared, so a reader of a Git-backed
    /// repository sees the object it has always seen.
    #[serde(skip_serializing_if = "Option::is_none")]
    comparison_unavailable: Option<String>,
    views: Vec<ViewStatus>,
}

#[derive(serde::Serialize)]
struct ViewStatus {
    name: String,
    path: String,
    remote: String,
    branch: String,
    anchor_source: Option<String>,
    anchor_view: Option<String>,
    anchor_tree_matches: Option<bool>,
    /// The view tip this repository derives to: a hash in the published
    /// repository's numbering.
    local_commit: Option<String>,
    published_commit: Option<String>,
    ahead: Option<usize>,
    behind: Option<usize>,
    elided: Option<usize>,
    state: ViewState,
    /// What the bookmark the views would be derived from points at, in this
    /// repository's own ids.
    ///
    /// A different quantity from `local_commit`, which is a Git hash of
    /// derived history, and carried in its own field for that reason. Only
    /// present when the views could not be compared, which is the only time
    /// the two could be confused for each other.
    #[serde(skip_serializing_if = "Option::is_none")]
    source_bookmark_commit: Option<String>,
    /// What a Git-backed clone last recorded for this view.
    ///
    /// True as of that survey, not of now, which is why it is a nested object
    /// rather than values in `ahead` and `behind`.
    #[serde(skip_serializing_if = "Option::is_none")]
    last_survey: Option<LastSurvey>,
}

/// The counts of a survey some other checkout ran, as it left them.
#[derive(serde::Serialize)]
struct LastSurvey {
    bookmark: String,
    anchor: String,
    upstream: String,
    behind: usize,
    ahead: usize,
}

#[derive(serde::Serialize)]
#[serde(rename_all = "snake_case")]
enum ViewState {
    UpToDate,
    LocalAhead,
    FastForward,
    Diverged,
    /// This repository cannot position the view against its published
    /// repository. Not a fifth position: the absence of one.
    NotCompared,
}

#[instrument(skip_all)]
pub async fn cmd_views_status(
    ui: &mut Ui,
    command: &CommandHelper,
    args: &ViewsStatusArgs,
) -> Result<(), CommandError> {
    let workspace_command = command.workspace_helper(ui).await?;
    let configured = get_views_config(&workspace_command).await?;
    let selected_names = args
        .upstream
        .as_ref()
        .map(std::slice::from_ref)
        .unwrap_or(&args.views);
    let selected = select_views(&configured, selected_names)?;
    // Before `anchor`, which reads the bookmark's target as a Git object id.
    // On a backend that keeps no Git objects it is not one, so the question of
    // whether this repository can compare at all has to be settled first.
    let Some((git, mut repo)) = try_open_store(&workspace_command)? else {
        return report_without_comparison(ui, &workspace_command, &selected, args);
    };
    let (_, local) = anchor(&workspace_command, &args.bookmark, "status")?;
    let mut cache = Cache::new();
    validate_endpoints(&git, &selected)?;
    if args.no_fetch {
        require_tracking_refs(&repo, &selected, args.upstream.is_some())?;
    }

    let freshness = if args.no_fetch {
        Freshness::AsOfLastFetch
    } else {
        Freshness::Fetch
    };
    let mut surveys = Vec::with_capacity(selected.len());
    for view in &selected {
        let survey = survey(
            &git,
            &mut repo,
            view,
            &local,
            args.upstream.is_some(),
            freshness,
            &mut cache,
        )?;
        record::write(
            workspace_command.repo_path(),
            &args.bookmark,
            &local,
            &survey,
        )?;
        surveys.push(survey);
    }
    if args.json {
        let output = StatusOutput {
            comparison_unavailable: None,
            views: surveys.iter().map(ViewStatus::from).collect(),
        };
        let json = serde_json::to_string(&output)
            .map_err(|err| user_error_with_message("Could not encode view status", err))?;
        writeln!(ui.stdout(), "{json}")?;
    } else {
        ui.request_pager();
        let mut out = ui.stdout();
        for survey in &surveys {
            writeln!(out, "{}: {}", survey.view.name, headline(survey))?;
            for line in detail(survey) {
                writeln!(out, "  {line}")?;
            }
        }
    }
    Ok(())
}

/// Reports the views without positioning them, for a repository whose backend
/// keeps no Git objects.
///
/// Three of the four things this command tells a reader survive the missing
/// capability: the manifest entry, what the bookmark the views derive from
/// points at, and the counts a Git-backed clone last recorded. Only the live
/// comparison needs Git. Aborting on the fourth threw away the other three,
/// which is how a repository with a readable `.jj-views.toml` came to answer
/// every question about it with `The repo is not backed by a Git repo`.
///
/// `jj views tree` already draws such a repository, dropping the publication
/// markers and saying in its own comment that this command is where the reason
/// belongs. This is that reason, and the exit status is zero because a report
/// that says what it does not know has reported successfully. `jj views check`
/// is the gate, and it still refuses.
fn report_without_comparison(
    ui: &mut Ui,
    workspace_command: &WorkspaceCommandHelper,
    selected: &[&ViewConfig],
    args: &ViewsStatusArgs,
) -> Result<(), CommandError> {
    let backend = workspace_command.repo().store().backend().name();
    let reason = format!(
        "the {backend} backend keeps no Git objects, so the published repositories cannot be \
         compared from here"
    );
    let source = local_bookmark_commit(workspace_command, &args.bookmark);
    let surveyed: Vec<Option<record::SurveyRecord>> = selected
        .iter()
        .map(|view| record::read(workspace_command.repo_path(), &view.name))
        .collect();

    if args.json {
        let views = selected
            .iter()
            .zip(&surveyed)
            .map(|(view, last)| ViewStatus {
                name: view.name.clone(),
                path: view.path.clone(),
                remote: view.remote.clone(),
                branch: view.branch.clone(),
                anchor_source: view.anchor.map(|anchor| anchor.source.to_string()),
                anchor_view: view.anchor.map(|anchor| anchor.view.to_string()),
                anchor_tree_matches: None,
                local_commit: None,
                published_commit: None,
                ahead: None,
                behind: None,
                elided: None,
                state: ViewState::NotCompared,
                source_bookmark_commit: source.as_ref().map(|id| id.hex()),
                last_survey: last.as_ref().map(|record| LastSurvey {
                    bookmark: record.bookmark.clone(),
                    anchor: record.anchor.clone(),
                    upstream: record.upstream.clone(),
                    behind: record.incoming,
                    ahead: record.ahead,
                }),
            })
            .collect();
        let output = StatusOutput {
            comparison_unavailable: Some(reason),
            views,
        };
        let json = serde_json::to_string(&output)
            .map_err(|err| user_error_with_message("Could not encode view status", err))?;
        writeln!(ui.stdout(), "{json}")?;
        return Ok(());
    }

    ui.request_pager();
    let mut out = ui.stdout();
    for (view, last) in selected.iter().zip(&surveyed) {
        writeln!(out, "{}: not compared; {reason}.", view.name)?;
        for line in uncompared_detail(view, source.as_ref(), last.as_ref(), &args.bookmark) {
            writeln!(out, "  {line}")?;
        }
    }
    Ok(())
}

/// The lines under an uncompared view's headline: everything this repository
/// does know about it.
fn uncompared_detail(
    view: &ViewConfig,
    source: Option<&CommitId>,
    last: Option<&record::SurveyRecord>,
    bookmark: &str,
) -> Vec<String> {
    let mut lines = vec![
        format!("here: {}", view.path),
        format!("{}: {} (never read)", view.remote, view.branch),
    ];
    if let Some(upstream) = &view.upstream {
        lines.push(format!(
            "upstream: {} ({})",
            upstream.remote, upstream.branch
        ));
    }
    if let Some(anchor) = view.anchor {
        lines.push(format!("anchor: {} -> {}", anchor.source, anchor.view));
    }
    match source {
        Some(commit) => lines.push(format!("{bookmark} is at {}", commit.hex())),
        // Every count would be missing for this reason as well as the backend's,
        // and a reader chasing the backend would not find it.
        None => lines.push(format!("{bookmark} does not exist here")),
    }
    match last {
        Some(record) => lines.push(format!(
            "as of the last survey, from {} at {}: {} behind, {} ahead of {}.",
            record.bookmark, record.anchor, record.incoming, record.ahead, record.upstream
        )),
        None => lines.push(
            "never surveyed here, so `jj views prompt` has no counts to show either.".to_owned(),
        ),
    }
    lines
}

impl From<&Survey<'_>> for ViewStatus {
    fn from(survey: &Survey<'_>) -> Self {
        let (anchor_source, anchor_view, anchor_tree_matches) = match survey.view.anchor {
            Some(anchor) => (
                Some(anchor.source.to_string()),
                Some(anchor.view.to_string()),
                Some(true),
            ),
            None => (None, None, None),
        };
        Self {
            name: survey.view.name.clone(),
            path: survey.view.path.clone(),
            remote: survey.remote.to_owned(),
            branch: survey.branch.to_owned(),
            anchor_source,
            anchor_view,
            anchor_tree_matches,
            local_commit: survey.derived.map(|id| id.to_string()),
            published_commit: Some(survey.upstream.to_string()),
            ahead: Some(survey.ahead),
            behind: Some(survey.incoming.len()),
            elided: Some(survey.elided),
            state: match survey.position {
                Position::Current => ViewState::UpToDate,
                Position::LocalAhead => ViewState::LocalAhead,
                Position::FastForward => ViewState::FastForward,
                Position::Diverged => ViewState::Diverged,
            },
            source_bookmark_commit: None,
            last_survey: None,
        }
    }
}

/// The one-line answer to "where is this view".
fn headline(survey: &Survey) -> String {
    let remote = survey.remote;
    match survey.position {
        Position::Current => "up to date.".to_owned(),
        Position::LocalAhead => format!(
            "{} ahead of {remote}; run `jj views push`.",
            commits(survey.ahead)
        ),
        Position::FastForward => format!(
            "{} behind {remote}; run `jj views fetch`.",
            commits(survey.incoming.len())
        ),
        Position::Diverged => format!(
            "diverged from {remote}; run `jj views fetch` to bring the published commits in \
             beside this history, then `jj new`."
        ),
    }
}

/// The lines under the headline, each one a number the headline cannot carry.
fn detail(survey: &Survey) -> Vec<String> {
    let mut lines = Vec::new();
    match survey.derived {
        Some(tip) => lines.push(format!("here: {tip} (from {})", survey.view.path)),
        // Nothing under the prefix in this bookmark's ancestry. Worth its own
        // line: every count below is then trivially zero for a reason that has
        // nothing to do with the published repository.
        None => lines.push(format!(
            "here: nothing under {} yet, so a fetch would import the whole history.",
            survey.view.path
        )),
    }
    lines.push(format!(
        "{}: {} ({})",
        survey.remote, survey.upstream, survey.branch
    ));
    if survey.ahead > 0 && survey.position != Position::LocalAhead {
        lines.push(format!("{} here not published yet.", commits(survey.ahead)));
    }
    if survey.elided > 0 {
        lines.push(elided_note(survey.elided));
    }
    lines
}
