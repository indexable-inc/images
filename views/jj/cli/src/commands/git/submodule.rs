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

mod clone;

use std::collections::BTreeMap;
use std::sync::Arc;

use clap::Subcommand;
use clap_complete::ArgValueCandidates;
use clap_complete::ArgValueCompleter;
use jj_lib::backend::BackendError;
use jj_lib::backend::CommitId;
use jj_lib::backend::TreeValue;
use jj_lib::git_submodule::SubmoduleConfig;
use jj_lib::git_submodule::read_gitmodules;
use jj_lib::merged_tree::MergedTree;
use jj_lib::object_id::ObjectId as _;
use jj_lib::repo::Repo as _;
use jj_lib::repo_path::RepoPathBuf;
use jj_lib::submodule_store::SubmoduleName;
use jj_lib::submodule_store::SubmoduleStore;
use jj_lib::submodule_store::load_submodule_repo;

use self::clone::GitSubmoduleCloneArgs;
use self::clone::cmd_git_submodule_clone;
use crate::cli_util::CommandHelper;
use crate::cli_util::RevisionArg;
use crate::command_error::CommandError;
use crate::command_error::user_error;
use crate::command_error::user_error_with_message;
use crate::complete;
use crate::generic_templater;
use crate::generic_templater::GenericTemplateLanguage;
use crate::templater::TemplatePropertyExt as _;
use crate::templater::TemplateRenderer;
use crate::ui::Ui;

/// Commands for working with Git submodules
///
/// Submodule support is being built incrementally, as sketched in
/// `docs/design/git-submodules.md`. Only a small part of it exists, so this
/// command tree is hidden the way `jj debug` and `jj bench` are: it is here to
/// be developed against, not to be relied on.
#[derive(Subcommand, Clone, Debug)]
#[command(hide = true)]
pub enum GitSubmoduleCommand {
    Clone(GitSubmoduleCloneArgs),
    List(GitSubmoduleListArgs),
    Status(GitSubmoduleStatusArgs),
}

pub async fn cmd_git_submodule(
    ui: &mut Ui,
    command: &CommandHelper,
    subcommand: &GitSubmoduleCommand,
) -> Result<(), CommandError> {
    match subcommand {
        GitSubmoduleCommand::Clone(args) => cmd_git_submodule_clone(ui, command, args).await,
        GitSubmoduleCommand::List(args) => cmd_git_submodule_list(ui, command, args).await,
        GitSubmoduleCommand::Status(args) => cmd_git_submodule_status(ui, command, args).await,
    }
}

/// List the submodules a revision's `.gitmodules` declares
///
/// This reports the contents of one file and nothing else, so a submodule
/// appears here whether or not it has ever been cloned, and a submodule that
/// has been cloned does not appear unless this revision still declares it. Use
/// `jj git submodule status` to compare the declaration against what jj
/// actually holds.
#[derive(clap::Args, Clone, Debug)]
pub struct GitSubmoduleListArgs {
    /// The revision whose `.gitmodules` to read
    #[arg(long, short, default_value = "@", value_name = "REVSET")]
    #[arg(add = ArgValueCompleter::new(complete::revset_expression_all))]
    revision: RevisionArg,

    /// Render each submodule using the given template
    ///
    /// The following keywords are available in the template expression:
    ///
    /// * `name: String`: The `<name>` in `[submodule "<name>"]`.
    /// * `path: String`: `submodule.<name>.path`, slash-separated and relative
    ///   to the repository root.
    /// * `url: String`: `submodule.<name>.url`, as written in the file, so it
    ///   may be relative to the superproject's remote.
    /// * `branch: String`: `submodule.<name>.branch`, or "" if unset. A branch
    ///   of `.` means the branch the superproject has checked out.
    /// * `update: String`: `submodule.<name>.update`, or "" if unset.
    ///
    /// Use `json(self)` for machine-readable output; an unset field is `null`
    /// there rather than "".
    ///
    /// Can be overridden by the `templates.git_submodule_list` setting.
    ///
    /// See [`jj help -k templates`] for more information.
    ///
    /// [`jj help -k templates`]:
    ///     https://docs.jj-vcs.dev/latest/templates/
    #[arg(long, short = 'T', verbatim_doc_comment)]
    #[arg(add = ArgValueCandidates::new(complete::template_aliases))]
    template: Option<String>,
}

/// One `[submodule "<name>"]` section, as `jj git submodule list` reports it.
#[derive(Clone, Debug, serde::Serialize)]
pub struct SubmoduleListItem {
    name: String,
    path: String,
    url: String,
    branch: Option<String>,
    update: Option<String>,
}

impl SubmoduleListItem {
    fn new(config: &SubmoduleConfig) -> Self {
        Self {
            name: config.name.as_str().to_owned(),
            path: config.path.as_internal_file_string().to_owned(),
            // `gix::Url` keeps the bytes it was parsed from, so this is what
            // `.gitmodules` says and not a normalized form of it. A relative
            // url is resolved against the superproject's remote, which this
            // command has no business guessing at.
            url: config.url.to_bstring().to_string(),
            branch: config.branch.as_ref().map(branch_to_string),
            update: config.update.as_ref().map(update_to_string),
        }
    }
}

fn branch_to_string(branch: &gix::submodule::config::Branch) -> String {
    match branch {
        // Written back in the `.gitmodules` spelling rather than resolved to
        // the superproject's current branch, because this command reports the
        // file rather than interpreting it.
        gix::submodule::config::Branch::CurrentInSuperproject => ".".to_owned(),
        gix::submodule::config::Branch::Name(name) => name.to_string(),
    }
}

fn update_to_string(update: &gix::submodule::config::Update) -> String {
    match update {
        gix::submodule::config::Update::Checkout => "checkout".to_owned(),
        gix::submodule::config::Update::Rebase => "rebase".to_owned(),
        gix::submodule::config::Update::Merge => "merge".to_owned(),
        gix::submodule::config::Update::None => "none".to_owned(),
        // `GitmodulesFile::parse` rejects `!command` instead of accepting it,
        // so a value read from a `.gitmodules` blob never lands here. It is
        // still spelled the way the file spells it, so that this stays correct
        // if a caller ever layers an override that does allow it.
        gix::submodule::config::Update::Command(command) => format!("!{command}"),
    }
}

type SubmoduleListTemplateLanguage = GenericTemplateLanguage<'static, SubmoduleListItem>;

generic_templater::impl_self_property_wrapper!(SubmoduleListItem);

fn submodule_list_template_language(command: &CommandHelper) -> SubmoduleListTemplateLanguage {
    let mut language = SubmoduleListTemplateLanguage::new(command.settings(), command.cwd());
    language.add_keyword("name", |self_property| {
        let out_property = self_property.map(|item| item.name);
        Ok(out_property.into_dyn_wrapped())
    });
    language.add_keyword("path", |self_property| {
        let out_property = self_property.map(|item| item.path);
        Ok(out_property.into_dyn_wrapped())
    });
    language.add_keyword("url", |self_property| {
        let out_property = self_property.map(|item| item.url);
        Ok(out_property.into_dyn_wrapped())
    });
    // The template language has no optional string, so an unset key is "".
    // `json(self)` is the way to tell "" apart from unset.
    language.add_keyword("branch", |self_property| {
        let out_property = self_property.map(|item| item.branch.unwrap_or_default());
        Ok(out_property.into_dyn_wrapped())
    });
    language.add_keyword("update", |self_property| {
        let out_property = self_property.map(|item| item.update.unwrap_or_default());
        Ok(out_property.into_dyn_wrapped())
    });
    language
}

async fn cmd_git_submodule_list(
    ui: &mut Ui,
    command: &CommandHelper,
    args: &GitSubmoduleListArgs,
) -> Result<(), CommandError> {
    let workspace_command = command.workspace_helper(ui).await?;
    let commit = workspace_command
        .resolve_single_rev(ui, &args.revision)
        .await?;
    let gitmodules = read_gitmodules(&commit.tree())
        .await
        .map_err(|err| user_error_with_message("Failed to read .gitmodules", err))?
        .unwrap_or_default();

    let template: TemplateRenderer<SubmoduleListItem> = {
        let language = submodule_list_template_language(command);
        let text = match &args.template {
            Some(value) => value.to_owned(),
            None => command
                .settings()
                .get_string("templates.git_submodule_list")?,
        };
        command
            .parse_template(ui, &language, &text)?
            .labeled(["git_submodule_list"])
    };

    ui.request_pager();
    let mut formatter = ui.stdout_formatter();
    for config in &gitmodules {
        template.format(&SubmoduleListItem::new(config), formatter.as_mut())?;
    }
    Ok(())
}

/// Compare a revision's `.gitmodules` against the submodules jj holds
///
/// The submodule store is the source of truth for what jj has; `.gitmodules`
/// and the gitlinks in a revision's tree say what that revision expects. The
/// three disagree routinely, and this command reports how, one line per
/// submodule, without trying to reconcile them.
///
/// A submodule can be in several of these situations at once, in which case the
/// first one that applies is reported, in this order:
///
/// * `no-gitlink`: `.gitmodules` declares the submodule, but this revision's
///   tree has no gitlink at its path. The declaration is left over from another
///   revision, so there is nothing here to clone or fetch.
/// * `not-cloned`: declared and checked out by a gitlink, but the submodule
///   store has no repository for it.
/// * `not-fetched`: declared and cloned, but the store does not have the commit
///   the gitlink points at.
/// * `ok`: declared, cloned, and the gitlink's commit is in the store.
///
/// Two more states have no `.gitmodules` entry at all, so jj cannot tell
/// whether they are the same submodule seen from two sides, and reports them as
/// separate lines:
///
/// * `undeclared-repo`: the store has a repository under this name, but this
///   revision declares no submodule by that name. Usually the submodule was
///   removed, or was added by a revision that is not this one.
/// * `undeclared-gitlink`: the tree has a gitlink at this path that
///   `.gitmodules` does not declare. Git permits this and it is a common broken
///   state; `git submodule` will not act on such a gitlink either.
#[derive(clap::Args, Clone, Debug)]
#[command(verbatim_doc_comment)]
pub struct GitSubmoduleStatusArgs {
    /// The revision to compare the submodule store against
    #[arg(long, short, default_value = "@", value_name = "REVSET")]
    #[arg(add = ArgValueCompleter::new(complete::revset_expression_all))]
    revision: RevisionArg,

    /// Render each submodule using the given template
    ///
    /// The following keywords are available in the template expression:
    ///
    /// * `state: String`: One of the states listed above.
    /// * `name: String`: The submodule's name, or "" for `undeclared-gitlink`,
    ///   which has no `.gitmodules` section to take a name from.
    /// * `path: String`: Where the submodule is checked out, or "" for
    ///   `undeclared-repo`, which this revision does not place anywhere.
    /// * `url: String`: `submodule.<name>.url`, or "" when this revision does
    ///   not declare the submodule.
    /// * `commit_id: String`: The submodule commit the gitlink records, or ""
    ///   when the tree has no gitlink for this submodule.
    ///
    /// Use `json(self)` for machine-readable output; an unset field is `null`
    /// there rather than "".
    ///
    /// Can be overridden by the `templates.git_submodule_status` setting.
    ///
    /// See [`jj help -k templates`] for more information.
    ///
    /// [`jj help -k templates`]:
    ///     https://docs.jj-vcs.dev/latest/templates/
    #[arg(long, short = 'T', verbatim_doc_comment)]
    #[arg(add = ArgValueCandidates::new(complete::template_aliases))]
    template: Option<String>,
}

/// How a submodule's declaration, stored repository and gitlink line up.
///
/// Each variant is a distinct thing to do about it, which is why they are not
/// folded into one "out of sync": a submodule that was never cloned needs a
/// clone, one that was never fetched needs a fetch, and one the store holds but
/// nothing declares needs neither.
#[derive(Clone, Copy, Debug, Eq, PartialEq, serde::Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum SubmoduleState {
    Ok,
    NoGitlink,
    NotCloned,
    NotFetched,
    UndeclaredRepo,
    UndeclaredGitlink,
}

impl SubmoduleState {
    fn as_str(self) -> &'static str {
        match self {
            Self::Ok => "ok",
            Self::NoGitlink => "no-gitlink",
            Self::NotCloned => "not-cloned",
            Self::NotFetched => "not-fetched",
            Self::UndeclaredRepo => "undeclared-repo",
            Self::UndeclaredGitlink => "undeclared-gitlink",
        }
    }
}

/// One line of `jj git submodule status`.
#[derive(Clone, Debug, serde::Serialize)]
pub struct SubmoduleStatusItem {
    state: SubmoduleState,
    name: Option<String>,
    path: Option<String>,
    url: Option<String>,
    commit_id: Option<String>,
}

type SubmoduleStatusTemplateLanguage = GenericTemplateLanguage<'static, SubmoduleStatusItem>;

generic_templater::impl_self_property_wrapper!(SubmoduleStatusItem);

fn submodule_status_template_language(command: &CommandHelper) -> SubmoduleStatusTemplateLanguage {
    let mut language = SubmoduleStatusTemplateLanguage::new(command.settings(), command.cwd());
    language.add_keyword("state", |self_property| {
        let out_property = self_property.map(|item| item.state.as_str().to_owned());
        Ok(out_property.into_dyn_wrapped())
    });
    // The template language has no optional string, so a field that does not
    // apply to this state is "". `json(self)` is the way to tell "" apart from
    // a field that genuinely is empty.
    language.add_keyword("name", |self_property| {
        let out_property = self_property.map(|item| item.name.unwrap_or_default());
        Ok(out_property.into_dyn_wrapped())
    });
    language.add_keyword("path", |self_property| {
        let out_property = self_property.map(|item| item.path.unwrap_or_default());
        Ok(out_property.into_dyn_wrapped())
    });
    language.add_keyword("url", |self_property| {
        let out_property = self_property.map(|item| item.url.unwrap_or_default());
        Ok(out_property.into_dyn_wrapped())
    });
    language.add_keyword("commit_id", |self_property| {
        let out_property = self_property.map(|item| item.commit_id.unwrap_or_default());
        Ok(out_property.into_dyn_wrapped())
    });
    language
}

async fn cmd_git_submodule_status(
    ui: &mut Ui,
    command: &CommandHelper,
    args: &GitSubmoduleStatusArgs,
) -> Result<(), CommandError> {
    let workspace_command = command.workspace_helper(ui).await?;
    let commit = workspace_command
        .resolve_single_rev(ui, &args.revision)
        .await?;
    let tree = commit.tree();
    let declared = read_gitmodules(&tree)
        .await
        .map_err(|err| user_error_with_message("Failed to read .gitmodules", err))?
        .unwrap_or_default();
    let gitlinks = collect_gitlinks(&tree)?;
    let store = workspace_command.repo().submodule_store();

    let mut items = Vec::new();
    // One line per declared submodule, which is the only source that can tie a
    // name to a path, and so the only one that can line the other two up.
    for config in &declared {
        let gitlink = gitlinks.get(&*config.path);
        let state = match gitlink {
            None => SubmoduleState::NoGitlink,
            Some(commit_id) => {
                if store
                    .contains(&config.name)
                    .map_err(|err| user_error_with_message("Failed to read submodule store", err))?
                {
                    let fetched =
                        submodule_repo_has_commit(command, store, &config.name, commit_id).await?;
                    if fetched {
                        SubmoduleState::Ok
                    } else {
                        SubmoduleState::NotFetched
                    }
                } else {
                    SubmoduleState::NotCloned
                }
            }
        };
        items.push(SubmoduleStatusItem {
            state,
            name: Some(config.name.as_str().to_owned()),
            path: Some(config.path.as_internal_file_string().to_owned()),
            url: Some(config.url.to_bstring().to_string()),
            commit_id: gitlink.map(|id| id.hex()),
        });
    }

    // One line per stored repository this revision does not declare. The store
    // is keyed by name and a gitlink only gives a path, so there is no way to
    // pair one of these with an undeclared gitlink even when both are present;
    // saying so as two lines beats guessing.
    let stored = store
        .list()
        .map_err(|err| user_error_with_message("Failed to read submodule store", err))?;
    for name in stored {
        if declared.get(name.as_str()).is_some() {
            continue;
        }
        items.push(SubmoduleStatusItem {
            state: SubmoduleState::UndeclaredRepo,
            name: Some(name.into_string()),
            path: None,
            url: None,
            commit_id: None,
        });
    }

    // One line per gitlink no `[submodule]` section claims. Git allows this,
    // and its own `git submodule` commands quietly skip such a gitlink, which
    // is how a repository ends up in this state without anyone noticing.
    for (path, commit_id) in &gitlinks {
        if declared.get_by_path(path).is_some() {
            continue;
        }
        items.push(SubmoduleStatusItem {
            state: SubmoduleState::UndeclaredGitlink,
            name: None,
            path: Some(path.as_internal_file_string().to_owned()),
            url: None,
            commit_id: Some(commit_id.hex()),
        });
    }

    // Named submodules first, in name order, then the nameless gitlinks in path
    // order, so that the output does not depend on which source was walked.
    items.sort_by(|left, right| {
        let key = |item: &SubmoduleStatusItem| {
            (item.name.is_none(), item.name.clone(), item.path.clone())
        };
        key(left).cmp(&key(right))
    });

    let template: TemplateRenderer<SubmoduleStatusItem> = {
        let language = submodule_status_template_language(command);
        let text = match &args.template {
            Some(value) => value.to_owned(),
            None => command
                .settings()
                .get_string("templates.git_submodule_status")?,
        };
        command
            .parse_template(ui, &language, &text)?
            .labeled(["git_submodule_status"])
    };

    ui.request_pager();
    let mut formatter = ui.stdout_formatter();
    for item in &items {
        template.format(item, formatter.as_mut())?;
    }
    Ok(())
}

/// The gitlinks in `tree`, keyed by the path they sit at.
///
/// A gitlink whose entry is conflicted is an error rather than a line of
/// output, matching `read_gitmodules`'s refusal to parse a conflicted
/// `.gitmodules`: there is no single submodule commit to look for, so any state
/// this command reported for it would be made up.
fn collect_gitlinks(tree: &MergedTree) -> Result<BTreeMap<RepoPathBuf, CommitId>, CommandError> {
    let mut gitlinks = BTreeMap::new();
    for (path, value) in tree.entries() {
        let value = value.map_err(|err| {
            user_error_with_message(
                format!(
                    "Failed to read tree entry {}",
                    path.as_internal_file_string()
                ),
                err,
            )
        })?;
        match value.as_resolved() {
            Some(Some(TreeValue::GitSubmodule(id))) => {
                gitlinks.insert(path, id.clone());
            }
            Some(_) => {}
            None => {
                if value
                    .iter()
                    .any(|term| matches!(term, Some(TreeValue::GitSubmodule(_))))
                {
                    return Err(user_error(format!(
                        "The Git submodule at {} is conflicted",
                        path.as_internal_file_string()
                    )));
                }
            }
        }
    }
    Ok(gitlinks)
}

/// Whether the repository stored for `name` has `commit_id`.
///
/// The submodule is driven as a whole repository rather than through its commit
/// backend, per `docs/design/git-submodule-storage.md`, so this loads the repo
/// the store holds and asks it.
async fn submodule_repo_has_commit(
    command: &CommandHelper,
    store: &Arc<dyn SubmoduleStore>,
    name: &SubmoduleName,
    commit_id: &CommitId,
) -> Result<bool, CommandError> {
    let repo_path = store
        .repo_path(name)
        .map_err(|err| user_error_with_message("Failed to read submodule store", err))?;
    let repo = load_submodule_repo(command.settings(), &repo_path, command.store_factories())
        .await
        .map_err(|err| {
            user_error_with_message(
                format!("Failed to load the repo for submodule '{name}'"),
                err,
            )
        })?;
    match repo.store().get_commit_async(commit_id).await {
        Ok(_) => Ok(true),
        Err(BackendError::ObjectNotFound { .. }) => Ok(false),
        Err(err) => Err(user_error_with_message(
            format!("Failed to look up a commit in submodule '{name}'"),
            err,
        )),
    }
}
