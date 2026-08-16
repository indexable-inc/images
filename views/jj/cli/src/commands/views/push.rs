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

use clap_complete::ArgValueCandidates;
use clap_complete::ArgValueCompleter;
use jj_lib::commit::Commit;
use jj_lib::object_id::ObjectId as _;
use jj_views::Cache;
use tracing::instrument;

use super::Freshness;
use super::Git;
use super::Position;
use super::ViewConfig;
use super::commit_tree;
use super::commits;
use super::get_views_config;
use super::open_store;
use super::select_views;
use super::survey;
use super::validate_views;
use crate::cli_util::CommandHelper;
use crate::cli_util::RevisionArg;
use crate::cli_util::WorkspaceCommandHelper;
use crate::command_error::CommandError;
use crate::command_error::user_error;
use crate::command_error::user_error_with_message;
use crate::complete;
use crate::ui::Ui;

/// Namespace the derived tip of each view is remembered under.
///
/// The ref is written before the push, not after, for two reasons: the objects
/// a derive produced are unreachable until something names them, so a failed
/// push would otherwise leave them for `git gc`; and a retry after a network
/// failure then costs nothing, because the derive is already cached in the
/// store rather than only in this process.
const VIEW_REF_NAMESPACE: &str = "refs/jj/views/";

/// Derive each configured view and push it to the repository it belongs to
///
/// A view is a path prefix of this repository that is also published as a
/// repository of its own. Deriving one produces a history whose hashes are
/// exactly the published repository's, so what this command sends is an
/// ordinary branch that repository can fast-forward.
///
/// By default the branch is a new one named after the revision's change ID, and
/// the command prints a URL to open a pull request from it. Writing a view's
/// own default branch takes `--branch` naming it *and*
/// `--allow-default-branch`.
///
/// This does not push the repository you are in. That push is `jj git push`,
/// which has bookmarks, tracking and force-with-lease semantics of its own, and
/// a second implementation of them here would be a second set of bugs. Run
/// both.
///
/// The command reads each view's published default branch before pushing. When
/// it exists, the branch is fetched for an exact comparison. An integrated
/// derived tip with the same root tree has no reviewable content, so its local
/// topology is recorded but no remote branch is created.
#[derive(clap::Args, Clone, Debug)]
pub struct ViewsPushArgs {
    /// View to push, by its key in the `views` config table (can be repeated)
    ///
    /// Defaults to every configured view.
    #[arg(value_name = "VIEW")]
    #[arg(add = ArgValueCandidates::new(complete::views))]
    views: Vec<String>,

    /// Revision whose view to push
    #[arg(long, short, default_value = "@", value_name = "REVSET")]
    #[arg(add = ArgValueCompleter::new(complete::revset_expression_all))]
    revision: RevisionArg,

    /// Branch to push under, instead of the generated name
    ///
    /// Use the `templates.git_push_bookmark` setting to customize the generated
    /// name. The default is `"push-" ++ change_id.short()`.
    #[arg(long, short, value_name = "NAME")]
    branch: Option<String>,

    /// Permit `--branch` to name a view's own default branch
    ///
    /// Pushing straight to the branch a published repository builds and
    /// releases from skips every review its own repository would have applied.
    /// It is occasionally what you want, and never what you want by accident.
    #[arg(long, requires = "branch")]
    allow_default_branch: bool,

    /// Replace a published branch that hash-drifted from what this repository
    /// derives
    ///
    /// A published repository can end up holding its own copies of commits this
    /// repository already derives: the same trees, under different commit
    /// objects, because both sides created them independently. Deriving is
    /// defined to produce the published repository's hashes, so the drifted
    /// copies are the wrong ones, and nothing that only moves a branch forward
    /// can remove them -- `jj views fetch` compares content, finds none
    /// missing, and reports the view up to date.
    ///
    /// This replaces them, and only them. The push is refused unless the
    /// published tip's tree is exactly the derived tip's, so a branch carrying
    /// content this repository does not produce is never overwritten by it. The
    /// replaced tip is written to `refs/pins/<date>-<hash>` in the same push,
    /// because lock files elsewhere may pin it.
    #[arg(long, requires = "branch")]
    replace_drifted: bool,

    /// Allow pushing a view whose tip commit has an empty description
    ///
    /// The description a view publishes is the one the monorepo commit carries,
    /// so an undescribed change here is an undescribed commit in a repository
    /// other people read.
    #[arg(long)]
    allow_empty_description: bool,

    /// Allow pushing a commit that changes paths outside the view
    ///
    /// A view filters the files it sends and copies the message through
    /// unchanged, because copying it is what makes a derived commit hash the
    /// published repository's own. A commit that spans the view and the rest of
    /// the repository therefore arrives with a correct file list and a message
    /// written about work that is not in it.
    #[arg(long)]
    allow_mixed: bool,

    /// Derive every view and report what would be pushed, without pushing
    #[arg(long)]
    dry_run: bool,
}

/// A published tip this push drops from the branch's history, and the ref that
/// keeps it reachable.
#[derive(Clone, Debug)]
struct Replacement {
    /// What the published branch pointed at before the push.
    replaced: gix::ObjectId,
    /// The `refs/pins/` ref written in the same push.
    pin: String,
}

/// One view, decided locally and ready to send.
struct Planned<'view> {
    view: &'view ViewConfig,
    tip: gix::ObjectId,
    /// The published tip, when the published branch already carries everything
    /// this view derives and there is nothing to send.
    published: Option<gix::ObjectId>,
    /// What this push replaces, when it replaces rather than extends.
    replacing: Option<Replacement>,
}

/// What happened to one view.
enum Outcome {
    /// The branch was written on the remote.
    Pushed {
        url: Option<String>,
        replaced: Option<Replacement>,
    },
    /// The remote branch was already this commit.
    Current { url: Option<String> },
    /// The derived tip adds no content to the integrated published branch.
    NoChanges { published: gix::ObjectId },
    /// `git push` refused or failed. Carries what it said.
    Failed(String),
    /// An earlier view failed, so this one was never sent.
    NotAttempted,
}

#[instrument(skip_all)]
pub async fn cmd_views_push(
    ui: &mut Ui,
    command: &CommandHelper,
    args: &ViewsPushArgs,
) -> Result<(), CommandError> {
    let workspace_command = command.workspace_helper(ui).await?;
    let settings = workspace_command.settings();
    let configured = get_views_config(&workspace_command).await?;
    let selected = select_views(&configured, &args.views)?;

    let commit = workspace_command
        .resolve_single_rev(ui, &args.revision)
        .await?;
    let branch = match &args.branch {
        Some(name) => name.clone(),
        None => generated_branch_name(ui, &workspace_command, &commit)?,
    };

    let (git, mut repo) = open_store(&workspace_command, "push")?;
    git.check_ref_format(&branch)?;

    // Everything that can be decided locally is decided before anything is
    // sent. Deriving is also the expensive half, so a repository that is not
    // going to be publishable fails without having written to any remote.
    for view in &selected {
        if branch == view.branch && !args.allow_default_branch {
            return Err(user_error(format!(
                "Refusing to push to {branch}, the default branch of the {} view at {}",
                view.name, view.remote
            ))
            .hinted(
                "Pass --allow-default-branch as well if that is really what you want, or drop \
                 --branch to push a new branch and open a pull request.",
            ));
        }
    }

    let source = gix::ObjectId::try_from(commit.id().as_bytes())
        .map_err(|err| user_error_with_message("Commit is not a Git object", err))?;
    let mut cache = Cache::new();
    validate_views(&git, &repo, &selected, &source, &mut cache)?;
    let mut derived = Vec::new();
    for view in &selected {
        let published_exists = git
            .remote_branch(&view.remote, &view.branch)
            .map_err(|err| user_error(format!("Could not read the {} view: {err}", view.name)))?
            .is_some();
        let mut upstream_tip = None;
        let (tip, published, replacing) = if published_exists {
            let surveyed = survey(
                &git,
                &mut repo,
                view,
                &source,
                false,
                Freshness::Fetch,
                &mut cache,
            )?;
            let tip = surveyed.derived.ok_or_else(|| no_history(view, args))?;
            // Decided here, with everything else that can be decided locally,
            // so a view that will be refused is refused before any view is
            // sent. Consulted even when the survey calls the view current:
            // hash drift IS current by content -- that is its definition --
            // so deciding by position alone made this command answer
            // "nothing to push" in exactly the state --replace-drifted
            // exists for (ENG-12041).
            let replacing = plan_replacement(
                &git,
                &repo,
                view,
                &branch,
                tip,
                surveyed.upstream,
                args.replace_drifted,
            )?;
            let published = (surveyed.position == Position::Current && replacing.is_none())
                .then_some(surveyed.upstream);
            upstream_tip = Some(surveyed.upstream);
            (tip, published, replacing)
        } else {
            let filter = view.filter()?;
            let tip = jj_views::derive(&repo, &source, &filter, &mut cache)
                .map_err(|err| {
                    user_error_with_message(format!("Could not derive {}", view.name), err)
                })?
                .ok_or_else(|| no_history(view, args))?;
            (tip, None, None)
        };
        if !args.allow_mixed {
            refuse_mixed(&repo, view, &source, upstream_tip, &mut cache)?;
        }
        if !args.allow_empty_description && has_empty_description(&repo, tip)? {
            return Err(user_error(format!(
                "Won't push the {} view: its tip {} has no description",
                view.name,
                tip.to_hex_with_len(12)
            ))
            .hinted(
                "Describe the revision the view derives from, or pass --allow-empty-description.",
            ));
        }
        repo.reference(
            format!("{VIEW_REF_NAMESPACE}{}", view.name),
            tip,
            gix::refs::transaction::PreviousValue::Any,
            "jj views push",
        )
        .map_err(|err| {
            user_error_with_message(format!("Could not record the {} view", view.name), err)
        })?;
        derived.push(Planned {
            view,
            tip,
            published,
            replacing,
        });
    }

    if args.dry_run {
        // The `refs/jj/views/` refs above were written even here. They are
        // local and hold nothing but what a derive already computed, and
        // keeping them is what makes the real push that follows a dry run cheap.
        let mut out = ui.status();
        for planned in &derived {
            let view = planned.view;
            let tip = planned.tip;
            match planned.published {
                Some(published) => writeln!(
                    out,
                    "{}: no content beyond {} at {published}; nothing to push.",
                    view.name, view.branch
                )?,
                None => {
                    if let Some(replacing) = &planned.replacing {
                        writeln!(
                            out,
                            "{}: would replace {} at {} on {}, pinning it as {}",
                            view.name, branch, replacing.replaced, view.remote, replacing.pin
                        )?;
                    }
                    writeln!(
                        out,
                        "{}: would push {tip} to {} as {branch}",
                        view.name, view.remote
                    )?;
                }
            }
        }
        writeln!(out, "Dry-run requested, not pushing.")?;
        return Ok(());
    }

    // A fixed order, and a stop at the first failure, so what landed is always
    // a prefix of that order rather than an arbitrary subset. See the report
    // below for what a partial push leaves behind.
    let mut outcomes = Vec::new();
    let mut failed = false;
    for planned in &derived {
        if failed {
            outcomes.push((planned.view, planned.tip, Outcome::NotAttempted));
            continue;
        }
        let outcome = match planned.published {
            Some(published) => Outcome::NoChanges { published },
            None => push_one(
                &git,
                planned.view,
                planned.tip,
                &branch,
                planned.replacing.as_ref(),
            ),
        };
        failed = matches!(outcome, Outcome::Failed(_));
        outcomes.push((planned.view, planned.tip, outcome));
    }

    report(ui, &outcomes, &branch)?;
    if failed {
        Err(user_error("Not every view was pushed").hinted(
            "The views listed above as pushed are on their remotes; re-run to send the rest.",
        ))
    } else {
        if settings.get_bool("hints.views-push-host-repo")? {
            writeln!(
                ui.hint_default(),
                "Only the views were pushed. Nothing in this repository moved: its own bookmarks, \
                 tags and remotes are exactly as they were, and `jj git push` is what sends those."
            )?;
        }
        Ok(())
    }
}

fn no_history(view: &ViewConfig, args: &ViewsPushArgs) -> CommandError {
    user_error(format!(
        "Nothing under {} anywhere in the ancestry of {}, so the {} view has no history to push",
        view.path, args.revision, view.name
    ))
}

/// Renders the same template `jj git push --change` uses.
///
/// Sharing it is the point: a branch this command creates and a branch that one
/// creates for the same revision have the same name, so a user who knows one
/// convention knows both.
fn generated_branch_name(
    ui: &Ui,
    workspace_command: &WorkspaceCommandHelper,
    commit: &Commit,
) -> Result<String, CommandError> {
    let text = workspace_command
        .settings()
        .get_string("templates.git_push_bookmark")?;
    let template = workspace_command.parse_commit_template(ui, &text)?;
    let output = template.format_plain_text(commit);
    let name = String::from_utf8(output).map_err(|err| {
        user_error_with_message("Invalid character in branch name", err.utf8_error())
    })?;
    if name.is_empty() {
        return Err(user_error("Empty branch name generated"));
    }
    Ok(name)
}

/// Sends one view, without forcing except over what we just observed.
fn push_one(
    git: &Git,
    view: &ViewConfig,
    tip: gix::ObjectId,
    branch: &str,
    replacing: Option<&Replacement>,
) -> Outcome {
    // Computed for both outcomes, not just the one that moved the branch. A
    // re-push is the common case for a change under review, and dropping the
    // link there sent people to the forge to find their own branch by hand.
    let url = pull_request_url(&view.remote, &view.branch, branch);
    let observed = match git.remote_branch(&view.remote, branch) {
        Ok(observed) => observed,
        Err(err) => return Outcome::Failed(err),
    };
    if observed == Some(tip) {
        return Outcome::Current { url };
    }
    // A default-branch push that plans no replacement was planned as a
    // fast-forward: `plan_replacement` proved the published tip it read is
    // inside the derived history. `observed` was read again just above, so a
    // branch that moved in between would otherwise be replaced under the
    // lease this command grants itself below, silently dropping whatever
    // landed there. Re-prove the fast-forward against what was actually
    // observed, and refuse anything else: dropping commits from a view's
    // default branch is what --replace-drifted exists to make explicit.
    if replacing.is_none() && branch == view.branch {
        if let Some(observed) = observed {
            if let Err(message) = fast_forward_only(git, observed, tip) {
                return Outcome::Failed(format!(
                    "refusing to move {branch} on {}: {message}",
                    view.remote
                ));
            }
        }
    }
    let pin = replacing.map(|replacing| (replacing.pin.as_str(), replacing.replaced));
    match git.push(&view.remote, tip, branch, observed, pin) {
        Ok(()) => Outcome::Pushed {
            url,
            replaced: replacing.cloned(),
        },
        Err(err) => Outcome::Failed(err),
    }
}

/// Proves that replacing `observed` with `tip` only extends history.
///
/// `Err` carries why it does not: either `observed` reaches commits `tip`
/// does not, or `observed` is a commit this repository has never seen, which
/// is a branch that moved somewhere unknown after this push was planned.
/// Either way the push would not fast-forward and must not be sent.
fn fast_forward_only(git: &Git, observed: gix::ObjectId, tip: gix::ObjectId) -> Result<(), String> {
    let dropped = git.history_count_after(&observed, &tip).map_err(|err| {
        format!(
            "the branch moved to {observed}, which this repository cannot position against the \
             derived tip {tip}: {err}"
        )
    })?;
    if dropped > 0 {
        return Err(format!(
            "the branch moved to {observed} after this push was planned, and {} there would be \
             dropped by the derived tip {tip}",
            commits(dropped)
        ));
    }
    Ok(())
}

/// Decides whether sending `tip` to `branch` replaces published history rather
/// than extending it, and whether this command is willing to.
///
/// Answers `None` for every push that is not a replacement: one to a branch
/// that is not the view's own, and one the published branch can fast-forward.
/// Those are sent exactly as they always were.
///
/// A replacement is only allowed for pure hash drift -- a published tip whose
/// tree this repository already derives, reached through commit objects it did
/// not produce. That is the one state no other command can reach: `jj views
/// fetch` compares content, finds nothing missing, and calls the view up to
/// date, while `jj views anchor` requires hash identity and fails. Anything
/// else is content this repository does not produce, and overwriting it would
/// destroy work rather than reconcile a copy of it.
fn plan_replacement(
    git: &Git,
    repo: &gix::Repository,
    view: &ViewConfig,
    branch: &str,
    tip: gix::ObjectId,
    published: gix::ObjectId,
    replace_drifted: bool,
) -> Result<Option<Replacement>, CommandError> {
    // Only the view's own default branch has history to replace. A push to a
    // branch of its own is a proposal, and re-deriving an amended revision is
    // expected to move it sideways.
    if branch != view.branch || tip == published {
        return Ok(None);
    }
    // Reachability, not "is the branch behind": a published tip the derived
    // history already contains is extended by this push, which is what an
    // ordinary push does.
    let dropped = git.history_count_after(&published, &tip).map_err(|err| {
        user_error(format!(
            "Could not compare the {} view with {}: {err}",
            view.name, view.remote
        ))
    })?;
    if dropped == 0 {
        return Ok(None);
    }

    let derived_tree = commit_tree(repo, tip)?;
    let published_tree = commit_tree(repo, published)?;
    if derived_tree != published_tree {
        return Err(user_error(format!(
            "Won't move {branch} on the {} view: {} has content this repository does not derive",
            view.name, view.remote
        ))
        .hinted(format!(
            "{} has {published} with tree {published_tree}, and this repository derives {tip} \
             with tree {derived_tree}. Run `jj views fetch {}` to bring the published commits in \
             here, integrate them, and push the result. --replace-drifted does not cover this: it \
             only replaces commits whose content this repository already produces.",
            view.remote, view.name
        )));
    }
    if !replace_drifted {
        return Err(user_error(format!(
            "Won't move {branch} on the {} view: {} is at {published}, which this repository's \
             derivation does not produce",
            view.name, view.remote
        ))
        .hinted(format!(
            "{published} and the derived tip {tip} have the same tree {derived_tree}, so the \
             published branch holds hash-drifted copies of commits this repository already \
             derives. `jj views fetch` compares content, so it reports the view up to date and \
             cannot reconcile this. Pass --replace-drifted to replace them; the tip they replace \
             is kept on a pin ref in the same push."
        )));
    }
    Ok(Some(Replacement {
        replaced: published,
        pin: pin_ref(published),
    }))
}

/// Where a replaced published tip is kept.
///
/// Lock files in other repositories pin published revisions by hash, so a
/// replaced tip that nothing names is a `flake.lock` that no longer resolves.
/// The day and the tip's own hash are both in the name so that two
/// replacements never collide, and so the ref says when and what without
/// having to be dereferenced.
fn pin_ref(replaced: gix::ObjectId) -> String {
    format!(
        "refs/pins/{}-{}",
        chrono::Local::now().format("%Y-%m-%d"),
        replaced.to_hex_with_len(12)
    )
}

/// Whether a derived commit says nothing about itself.
///
/// Only the tip is checked, not the whole derived history. Everything under it
/// is either already published, where refusing now is a refusal nobody can act
/// on, or was written before this repository adopted the check. The tip is the
/// commit this push adds and the one a reviewer opens.
/// Refuses to publish a commit whose message describes work the view drops.
///
/// The file list a view sends is filtered and its message is not, so a commit
/// that changed both the view and the rest of this repository publishes prose
/// about the half that stays behind. Filtering the files is not filtering the
/// commit.
///
/// Commits the published branch already carries are not re-reported. Their
/// messages are already out there, and a check that refuses forever once one
/// slips through is a check somebody turns off.
fn refuse_mixed(
    repo: &gix::Repository,
    view: &ViewConfig,
    source: &gix::ObjectId,
    upstream: Option<gix::ObjectId>,
    cache: &mut Cache,
) -> Result<(), CommandError> {
    let offenders = mixed_to_publish(repo, view, source, upstream, cache)?;
    let Some(first) = offenders.first() else {
        return Ok(());
    };
    let outside = join_paths(&first.outside);
    let short = first.commit.to_hex_with_len(12);
    let also = match offenders.len() {
        1 => String::new(),
        n => format!(
            " {} other commit{} would go the same way; `jj views check {}` lists them.",
            n - 1,
            if n == 2 { "" } else { "s" },
            view.name
        ),
    };
    Err(user_error(format!(
        "Won't push the {} view: commit {} changes {}, which the view does not carry",
        view.name, short, outside
    ))
    .hinted(format!(
        "The view sends that commit's message verbatim and only the files under {}, so the \
         message would reach {} describing work nobody there can see. Run `jj split -r {}` to \
         separate them, or pass --allow-mixed to publish it as it is.{}",
        view.path, view.remote, short, also
    )))
}

/// Mixed commits this push would add to the published branch, parents first.
fn mixed_to_publish(
    repo: &gix::Repository,
    view: &ViewConfig,
    source: &gix::ObjectId,
    upstream: Option<gix::ObjectId>,
    cache: &mut Cache,
) -> Result<Vec<jj_views::MixedCommit>, CommandError> {
    let filter = view.filter()?;
    let anchor = view.anchor.as_ref().map(|anchor| anchor.source);
    let mixed = jj_views::mixed_commits(
        repo,
        source,
        anchor.as_deref(),
        &filter,
        &super::exempt_paths(),
        cache,
    )
    .map_err(|err| super::lift_error(view, err))?;
    if mixed.is_empty() {
        return Ok(mixed);
    }
    let published = published_commits(repo, view, upstream)?;
    Ok(mixed
        .into_iter()
        .filter(|candidate| !published.contains(&candidate.derived))
        .collect())
}

/// Every view commit the published branch already reaches.
fn published_commits(
    repo: &gix::Repository,
    view: &ViewConfig,
    upstream: Option<gix::ObjectId>,
) -> Result<HashSet<gix::ObjectId>, CommandError> {
    let Some(upstream) = upstream else {
        return Ok(HashSet::new());
    };
    let mut out: HashSet<gix::ObjectId> = HashSet::new();
    // Bounded by the anchor for the same reason a derivation is: history older
    // than it is settled. A published branch that does not descend from the
    // anchor is a drift case other commands report, so fall back to the whole
    // walk rather than fail here.
    let walked = match view.anchor.as_ref() {
        Some(anchor) => jj_views::verify::ancestry_after(repo, &upstream, &anchor.view)
            .or_else(|_| jj_views::verify::ancestry(repo, &upstream)),
        None => jj_views::verify::ancestry(repo, &upstream),
    }
    .map_err(|err| super::lift_error(view, err))?;
    out.extend(walked);
    out.insert(upstream);
    if let Some(anchor) = view.anchor.as_ref() {
        out.insert(anchor.view);
    }
    Ok(out)
}

/// Renders a commit's outside paths for one line of a message.
fn join_paths(paths: &[bstr::BString]) -> String {
    const SHOWN: usize = 4;
    let listed = paths
        .iter()
        .take(SHOWN)
        .map(bstr::BString::to_string)
        .collect::<Vec<_>>()
        .join(", ");
    match paths.len().checked_sub(SHOWN) {
        Some(rest) if rest > 0 => format!("{listed} and {rest} more"),
        _ => listed,
    }
}

fn has_empty_description(repo: &gix::Repository, tip: gix::ObjectId) -> Result<bool, CommandError> {
    let object = repo.find_object(tip).map_err(|err| {
        user_error_with_message(format!("Could not read the derived commit {tip}"), err)
    })?;
    let commit = gix::objs::CommitRef::from_bytes(&object.data, repo.object_hash())
        .map_err(|err| user_error_with_message(format!("Could not read the commit {tip}"), err))?;
    Ok(commit.message.trim_ascii().is_empty())
}

fn report(
    ui: &Ui,
    outcomes: &[(&ViewConfig, gix::ObjectId, Outcome)],
    branch: &str,
) -> Result<(), CommandError> {
    for (view, tip, outcome) in outcomes {
        match outcome {
            Outcome::Pushed { url, replaced } => {
                if let Some(replaced) = replaced {
                    writeln!(
                        ui.status(),
                        "{}: replaced {branch} at {} on {}; pinned it as {}",
                        view.name,
                        replaced.replaced,
                        view.remote,
                        replaced.pin
                    )?;
                }
                writeln!(
                    ui.status(),
                    "{}: pushed {tip} to {} as {branch}",
                    view.name,
                    view.remote
                )?;
                report_pull_request_url(ui, &view.name, url.as_deref())?;
            }
            Outcome::Current { url } => {
                writeln!(
                    ui.status(),
                    "{}: {} already has {branch} at {tip}",
                    view.name,
                    view.remote
                )?;
                report_pull_request_url(ui, &view.name, url.as_deref())?;
            }
            Outcome::NoChanges { published } => {
                writeln!(
                    ui.status(),
                    "{}: no content beyond {} at {published}; nothing pushed.",
                    view.name,
                    view.branch
                )?;
            }
            Outcome::Failed(message) => {
                writeln!(
                    ui.warning_default(),
                    "{}: could not push to {}",
                    view.name,
                    view.remote
                )?;
                for line in message.lines() {
                    writeln!(ui.status(), "  {line}")?;
                }
            }
            Outcome::NotAttempted => {
                writeln!(ui.status(), "{}: not attempted.", view.name)?;
            }
        }
    }
    Ok(())
}

fn report_pull_request_url(ui: &Ui, name: &str, url: Option<&str>) -> Result<(), CommandError> {
    if let Some(url) = url {
        writeln!(ui.status(), "{name}: open a pull request at {url}")?;
    }
    Ok(())
}

/// The URL that opens a pull request from `head` into `base`, when the remote
/// is one whose shape we know.
///
/// Only GitHub, and deliberately: a guessed URL for a forge this does not
/// recognize is worse than none, because it looks like it was checked.
fn pull_request_url(remote: &str, base: &str, head: &str) -> Option<String> {
    // `--allow-default-branch` pushes straight to the base. There is no pull
    // request to open from a branch to itself, and GitHub renders the compare
    // page for one as an empty diff.
    if base == head {
        return None;
    }
    let repo = github_repo(remote)?;
    Some(format!(
        "https://github.com/{repo}/compare/{base}...{head}?expand=1"
    ))
}

/// `owner/name` for the three spellings of a GitHub remote.
fn github_repo(remote: &str) -> Option<&str> {
    let path = remote
        .strip_prefix("git@github.com:")
        .or_else(|| remote.strip_prefix("ssh://git@github.com/"))
        .or_else(|| remote.strip_prefix("https://github.com/"))
        .or_else(|| remote.strip_prefix("http://github.com/"))?;
    let path = path.strip_suffix(".git").unwrap_or(path);
    let path = path.trim_end_matches('/');
    // Exactly two components. Anything else is a URL shape this does not know,
    // and a compare link built from it would 404.
    (path.split('/').count() == 2 && !path.split('/').any(str::is_empty)).then_some(path)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recognizes_the_three_github_remote_spellings() {
        assert_eq!(
            github_repo("git@github.com:owner/repo.git"),
            Some("owner/repo")
        );
        assert_eq!(
            github_repo("ssh://git@github.com/owner/repo"),
            Some("owner/repo")
        );
        assert_eq!(
            github_repo("https://github.com/owner/repo.git"),
            Some("owner/repo")
        );
    }

    #[test]
    fn declines_to_guess_at_anything_else() {
        assert_eq!(github_repo("git@gitlab.com:owner/repo.git"), None);
        assert_eq!(github_repo("https://github.com/owner"), None);
        assert_eq!(github_repo("https://github.com/owner/repo/tree/main"), None);
        assert_eq!(github_repo("/srv/git/repo.git"), None);
    }

    #[test]
    fn offers_no_pull_request_from_the_base_to_itself() {
        assert_eq!(
            pull_request_url("git@github.com:indexable-inc/ix.git", "main", "main"),
            None
        );
    }

    fn fixture_commit(
        repo: &gix::Repository,
        tree: gix::ObjectId,
        parent: Option<gix::ObjectId>,
    ) -> gix::ObjectId {
        use gix::prelude::Write as _;
        let parent_line = parent
            .map(|parent| format!("parent {parent}\n"))
            .unwrap_or_default();
        let body = format!(
            "tree {tree}\n{parent_line}author t <t@t.invalid> 0 +0000\ncommitter t \
             <t@t.invalid> 0 +0000\n\nfixture\n"
        )
        .into_bytes();
        repo.objects
            .write_buf(gix::objs::Kind::Commit, &body)
            .expect("write fixture commit")
    }

    /// The guard behind every default-branch push that plans no replacement.
    ///
    /// A real store and a real `git rev-list`, because the guard's whole job
    /// is to re-ask Git about an id `ls-remote` returned after planning, and
    /// a mocked answer would prove nothing about that conversation.
    #[test]
    fn fast_forward_only_separates_extension_from_replacement() {
        use gix::prelude::Write as _;
        let scratch = tempfile::tempdir().expect("scratch");
        let store = scratch.path().join("store.git");
        gix::init_bare(&store).expect("init store");
        let repo = gix::open(&store).expect("open store");
        let tree = repo
            .objects
            .write_buf(gix::objs::Kind::Tree, b"")
            .expect("write empty tree");
        let root = fixture_commit(&repo, tree, None);
        let child = fixture_commit(&repo, tree, Some(root));
        let git = Git {
            executable: "git".into(),
            git_dir: store,
        };

        // The planned case: the observed tip is inside the derived history.
        fast_forward_only(&git, root, child).expect("an extension fast-forwards");

        // The race: the branch moved to a commit the derived tip does not
        // contain, so the push would drop it.
        let refusal = fast_forward_only(&git, child, root).expect_err("a drop is refused");
        assert!(
            refusal.contains("would be dropped"),
            "the refusal did not say what the push would drop: {refusal}"
        );

        // The branch moved to a commit this repository has never seen.
        let unknown = gix::ObjectId::from_hex(b"1de35c9e4b6a8f2d91c3e5a7b9047bd712345678")
            .expect("valid commit id");
        let refusal = fast_forward_only(&git, unknown, child).expect_err("unknown is refused");
        assert!(
            refusal.contains("cannot position"),
            "the refusal did not say the observed tip is unknown here: {refusal}"
        );
    }

    #[test]
    fn builds_a_compare_url_against_the_views_own_default_branch() {
        assert_eq!(
            pull_request_url(
                "git@github.com:indexable-inc/ix.git",
                "main",
                "push-qpvuntsm"
            ),
            Some(
                "https://github.com/indexable-inc/ix/compare/main...push-qpvuntsm?expand=1"
                    .to_owned()
            )
        );
    }
}
