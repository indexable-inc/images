// Copyright 2020 The Jujutsu Authors
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

use clap_complete::ArgValueCandidates;
use clap_complete::ArgValueCompleter;
use itertools::Itertools as _;
use jj_lib::backend::BackendResult;
use jj_lib::merge::MergedTreeValue;
use jj_lib::object_id::ObjectId as _;
use jj_lib::repo_path::RepoPathBuf;
use tracing::instrument;

use crate::cli_util::CommandHelper;
use crate::cli_util::RevisionArg;
use crate::cli_util::SubmoduleConflictKind;
use crate::cli_util::WorkspaceCommandHelper;
use crate::cli_util::print_conflicted_paths;
use crate::cli_util::print_unmatched_explicit_paths;
use crate::cli_util::submodule_conflict_kind;
use crate::command_error::CommandError;
use crate::command_error::cli_error;
use crate::command_error::user_error;
use crate::complete;
use crate::formatter::FormatterExt as _;
use crate::ui::Ui;

/// Resolve conflicted files with an external merge tool
///
/// Only conflicts that can be resolved with a 3-way merge are supported. See
/// docs for merge tool configuration instructions. External merge tools will be
/// invoked for each conflicted file one-by-one until all conflicts are
/// resolved. To stop resolving conflicts, exit the merge tool without making
/// any changes.
///
/// Note that conflicts can also be resolved without using this command. You may
/// edit the conflict markers in the conflicted file directly with a text
/// editor.
//  TODOs:
//   - `jj resolve --editor` to resolve a conflict in the default text editor. Should work for
//     conflicts with 3+ adds. Useful to resolve conflicts in a commit other than the current one.
//   - A way to help split commits with conflicts that are too complicated (more than two sides)
//     into commits with simpler conflicts. In case of a tree with many merges, we could for example
//     point to existing commits with simpler conflicts where resolving those conflicts would help
//     simplify the present one.
#[derive(clap::Args, Clone, Debug)]
pub(crate) struct ResolveArgs {
    #[arg(long, short, default_value = "@", value_name = "REVSET")]
    #[arg(add = ArgValueCompleter::new(complete::revset_expression_mutable_conflicts))]
    revision: RevisionArg,

    /// Instead of resolving conflicts, list all the conflicts
    // TODO: Also have a `--summary` option. `--list` currently acts like
    // `diff --summary`, but should be more verbose.
    #[arg(long, short)]
    list: bool,

    /// Specify 3-way merge tool to be used
    ///
    /// The built-in merge tools `:ours` and `:theirs` can be used to choose
    /// side #1 and side #2 of the conflict respectively.
    #[arg(long, conflicts_with = "list", value_name = "NAME")]
    #[arg(add = ArgValueCandidates::new(complete::merge_editors))]
    tool: Option<String>,

    /// Only resolve conflicts in these paths. You can use the `--list` argument
    /// to find paths to use here.
    #[arg(value_name = "FILESETS", value_hint = clap::ValueHint::AnyPath)]
    #[arg(add = ArgValueCompleter::new(complete::revision_conflicted_files))]
    paths: Vec<String>,
}

/// Conflicted paths that a Git submodule takes part in, in the order they were
/// listed, each paired with the UI form of its path.
fn find_submodule_conflicts(
    conflicts: &[(RepoPathBuf, BackendResult<MergedTreeValue>)],
    workspace_command: &WorkspaceCommandHelper,
) -> Vec<(String, SubmoduleConflictKind)> {
    conflicts
        .iter()
        .filter_map(|(path, conflict)| {
            // A path whose conflict could not even be read is reported by
            // print_conflicted_paths or by the merge tool, not here.
            let kind = submodule_conflict_kind(conflict.as_ref().ok()?)?;
            Some((workspace_command.format_file_path(path), kind))
        })
        .collect()
}

fn describe_submodule_conflict(ui_path: &str, kind: SubmoduleConflictKind) -> String {
    match kind {
        SubmoduleConflictKind::AllSubmodules => format!(
            "The conflict at {ui_path:?} is between Git submodule commits, which no merge tool \
             can resolve"
        ),
        SubmoduleConflictKind::Mixed => format!(
            "The path {ui_path:?} is a Git submodule on some sides of the conflict and not on \
             others, which no merge tool can resolve"
        ),
    }
}

fn submodule_resolution_advice(ui_path: &str) -> String {
    format!(
        "jj records only the submodule's commit id, never its contents, so there is nothing for a \
         merge tool to merge. Pick the side you want with `jj restore --from <revision> \
         {ui_path:?}`."
    )
}

#[instrument(skip_all)]
pub(crate) async fn cmd_resolve(
    ui: &mut Ui,
    command: &CommandHelper,
    args: &ResolveArgs,
) -> Result<(), CommandError> {
    let mut workspace_command = command.workspace_helper(ui).await?;
    let fileset_expression = workspace_command.parse_file_patterns(ui, &args.paths)?;
    let matcher = fileset_expression.to_matcher();
    let commit = workspace_command
        .resolve_single_rev(ui, &args.revision)
        .await?;
    let tree = commit.tree();
    let conflicts = tree.conflicts_matching(&matcher).collect_vec();

    print_unmatched_explicit_paths(ui, &workspace_command, &fileset_expression, [&tree])?;

    if conflicts.is_empty() {
        return Err(cli_error(if args.paths.is_empty() {
            "No conflicts found at this revision"
        } else {
            "No conflicts found at the given path(s)"
        }));
    }
    // jj records a submodule as a commit id and never reads its contents, so a
    // merge tool has nothing to merge. Diagnose this before a tool is picked,
    // rather than letting one start and fail on the first such path.
    let submodule_conflicts = find_submodule_conflicts(&conflicts, &workspace_command);

    if args.list {
        print_conflicted_paths(
            conflicts,
            ui.stdout_formatter().as_mut(),
            &workspace_command,
        )?;
        // Same words as the error `jj resolve` would give for these paths, so
        // the listing and the command agree on what can be done about them.
        for (path, kind) in &submodule_conflicts {
            let description = describe_submodule_conflict(path, *kind);
            let advice = submodule_resolution_advice(path);
            writeln!(ui.hint_default(), "{description}. {advice}")?;
        }
        return Ok(());
    }

    if let Some((path, kind)) = submodule_conflicts.first() {
        let mut err = user_error(describe_submodule_conflict(path, *kind));
        err.add_hint(submodule_resolution_advice(path));
        if submodule_conflicts.len() < conflicts.len() {
            err.add_hint(
                "Pass the paths of the other conflicted files to `jj resolve` to resolve them.",
            );
        }
        return Err(err);
    }

    let repo_paths = conflicts
        .iter()
        .map(|(path, _)| path.as_ref())
        .collect_vec();
    workspace_command.check_rewritable([commit.id()]).await?;
    let merge_editor = workspace_command.merge_editor(ui, args.tool.as_deref())?;
    let mut tx = workspace_command.start_transaction();
    let (new_tree, partial_resolution_error) =
        merge_editor.edit_files(ui, &tree, &repo_paths).await?;
    let new_commit = tx
        .repo_mut()
        .rewrite_commit(&commit)
        .set_tree(new_tree)
        .write()
        .await?;
    tx.finish(
        ui,
        format!("Resolve conflicts in commit {}", commit.id().hex()),
    )
    .await?;

    // Print conflicts that are still present after resolution if the workspace
    // working copy is not at the commit. Otherwise, the conflicting paths will
    // be printed by the `tx.finish()` instead.
    if workspace_command.get_wc_commit_id() != Some(new_commit.id())
        && let Some(mut formatter) = ui.status_formatter()
        && new_commit.has_conflict()
    {
        let new_tree = new_commit.tree();
        let new_conflicts = new_tree.conflicts().collect_vec();
        writeln!(
            formatter.labeled("warning").with_heading("Warning: "),
            "After this operation, some files at this revision still have conflicts:"
        )?;
        print_conflicted_paths(new_conflicts, formatter.as_mut(), &workspace_command)?;
    }

    if let Some(err) = partial_resolution_error {
        return Err(err.into());
    }
    Ok(())
}
