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

use std::cell::OnceCell;
use std::cell::RefCell;
use std::io::Write as _;
use std::path::Path;
use std::pin::Pin;
use std::rc::Rc;

use clap_complete::ArgValueCandidates;
use jj_lib::merged_tree::MergedTree;
use jj_lib::settings::UserSettings;
use jj_views::Cache;
use tracing::instrument;

use super::Freshness;
use super::Git;
use super::Position;
use super::ViewConfig;
use super::anchor;
use super::get_views_config;
use super::manifest_views_config;
use super::survey;
use super::try_open_store;
use super::working_copy_tree;
use crate::cli_util::CommandHelper;
use crate::cli_util::WorkspaceCommandHelper;
use crate::command_error::CommandError;
use crate::complete;
use crate::formatter::Formatter;
use crate::formatter::FormatterExt as _;
use crate::generic_templater;
use crate::generic_templater::GenericTemplateLanguage;
use crate::templater::TemplatePropertyExt as _;
use crate::templater::TemplateRenderer;
use crate::ui::Ui;

/// Draw the hierarchy of views reachable from this repository
///
/// Every other `views` command reads one manifest: the one at this
/// repository's root. But a view's own content can carry a `.jj-views.toml`
/// of its own, nesting views inside views, and those inner manifests are
/// exactly what the root one cannot see. This walks them all: each line is
/// one view, drawn under the manifest that declares it.
///
/// The default line is the view's name plus, when the store can answer
/// without the network, where the view stands against its published
/// repository: `⇡n` view commits here the published repository does not
/// have, `⇣n` published commits not integrated here, read against what the
/// last `jj views fetch` or `jj views status` recorded. A view with no such
/// record -- every nested one, and any top-level one never fetched -- shows
/// no markers, and `jj views status` is the command that goes and asks. The
/// endpoints themselves are one `-T builtin_views_tree_detailed` away.
#[derive(clap::Args, Clone, Debug)]
pub struct ViewsTreeArgs {
    /// Render each view using the given template
    ///
    /// The following keywords are available in the template expression:
    ///
    /// * `name: String`: The view's key in the `views` config table.
    /// * `path: String`: Path prefix in its repository that the view is of.
    /// * `remote: String`: URL the view is published to.
    /// * `branch: String`: The published repository's default branch.
    /// * `upstream_remote: String`: Read-only upstream URL, empty for a view
    ///   that tracks none.
    /// * `upstream_branch: String`: Read-only upstream branch, empty likewise.
    /// * `depth: Integer`: How many manifests deep the view is declared,
    ///   counting the root manifest as 0.
    /// * `tracked: Boolean`: True when this repository holds a last-fetched
    ///   publication state to compare against. Always false for nested views.
    /// * `ahead: Option<Integer>`: View commits here the published repository
    ///   does not have. Absent when not `tracked`.
    /// * `behind: Option<Integer>`: Published commits not integrated here.
    /// * `elided: Option<Integer>`: Published commits this repository holds as
    ///   commits the view drops.
    /// * `diverged: Boolean`: True when both sides have view commits the other
    ///   does not.
    ///
    /// Can be overridden by the `templates.views_tree` setting. To see the
    /// full endpoints, use the `builtin_views_tree_detailed` template.
    ///
    /// See [`jj help -k templates`] for more information.
    ///
    /// [`jj help -k templates`]:
    ///     https://docs.jj-vcs.dev/latest/templates/
    #[arg(long, short = 'T', verbatim_doc_comment)]
    #[arg(add = ArgValueCandidates::new(complete::template_aliases))]
    template: Option<String>,

    /// Bookmark the status markers measure the views from
    #[arg(long, short = 'b', default_value = "main", value_name = "NAME")]
    #[arg(add = ArgValueCandidates::new(complete::local_bookmarks))]
    bookmark: String,
}

#[instrument(skip_all)]
pub async fn cmd_views_tree(
    ui: &mut Ui,
    command: &CommandHelper,
    args: &ViewsTreeArgs,
) -> Result<(), CommandError> {
    let workspace_command = command.workspace_helper(ui).await?;
    let root = workspace_command.workspace_root();
    let configured = get_views_config(&workspace_command).await?;
    // Nested manifests come from the same commit as the root one, so a view
    // whose manifest this checkout does not materialize still gets walked.
    let tree = working_copy_tree(&workspace_command).await?;
    let nodes = collect_nested(tree.as_ref(), String::new(), configured).await?;

    let language = views_tree_template_language(command.settings(), command.cwd());
    let text = match &args.template {
        Some(value) => value.to_owned(),
        None => command.settings().get_string("templates.views_tree")?,
    };
    let template = command
        .parse_template(ui, &language, &text)?
        .labeled(["views_tree"]);

    // Opening the store and reading the bookmark tip is cheap and infallible
    // enough to do up front; the per-view comparison behind the status
    // keywords is what costs, and that stays inside [`LazyStatus`] so a
    // template that never asks never pays for it.
    let status = status_context(&workspace_command, &args.bookmark);

    ui.request_pager();
    let mut formatter = ui.stdout_formatter();
    writeln!(formatter, "{}", root.display())?;
    let mut writer = TreeWriter {
        formatter: formatter.as_mut(),
        template: &template,
    };
    writer.write_subtree(&nodes, "", 0, status.as_ref())
}

/// One view plus the views its own manifest declares.
///
/// The whole hierarchy is gathered before anything is written, because
/// reading a manifest out of the commit is async and the render below wants
/// to hold the formatter across the recursion.
struct Node {
    view: ViewConfig,
    children: Vec<Self>,
}

/// Reads each view's own manifest, and theirs, depth first.
///
/// `prefix` is where `views` were declared, so a nested manifest is looked up
/// at its full path in the repository rather than relative to a directory
/// that may not exist on disk.
fn collect_nested<'a>(
    tree: Option<&'a MergedTree>,
    prefix: String,
    views: Vec<ViewConfig>,
) -> Pin<Box<dyn Future<Output = Result<Vec<Node>, CommandError>> + 'a>> {
    Box::pin(async move {
        let mut nodes = Vec::with_capacity(views.len());
        for view in views {
            let path = view.path.trim_matches('/');
            let child = if prefix.is_empty() {
                path.to_owned()
            } else {
                format!("{prefix}/{path}")
            };
            let mut children = Vec::new();
            if let Some(tree) = tree
                && let Some(nested) = manifest_views_config(tree, &child).await?
            {
                children = collect_nested(Some(tree), child, nested).await?;
            }
            nodes.push(Node { view, children });
        }
        Ok(nodes)
    })
}

/// The output side of the walk: where the tree is drawn, and with what.
struct TreeWriter<'a, 'render> {
    formatter: &'a mut dyn Formatter,
    template: &'a TemplateRenderer<'render, TreeEntry>,
}

impl TreeWriter<'_, '_> {
    /// Writes one manifest's views, each followed by its own nested manifest.
    fn write_subtree(
        &mut self,
        nodes: &[Node],
        indent: &str,
        depth: i64,
        status: Option<&Rc<StatusContext>>,
    ) -> Result<(), CommandError> {
        for (index, node) in nodes.iter().enumerate() {
            let last = index == nodes.len() - 1;
            let arm = if last { "└─" } else { "├─" };
            {
                let mut scope = self.formatter.labeled("views_tree");
                write!(scope.labeled("tree"), "{indent}{arm} ")?;
            }
            let entry = TreeEntry::new(&node.view, depth, status);
            self.template.format(&entry, self.formatter)?;
            writeln!(self.formatter)?;
            if !node.children.is_empty() {
                let indent = format!("{indent}{}", if last { "   " } else { "│  " });
                // A nested view's publication state lives in its own
                // repository, not in this store, so its entries carry no
                // status source: absent is the honest answer, and nothing
                // here fetches or derives a nested repository to manufacture
                // one.
                self.write_subtree(&node.children, &indent, depth + 1, None)?;
            }
        }
        Ok(())
    }
}

/// One view as the template context sees it.
#[derive(Clone, serde::Serialize)]
struct TreeEntry {
    name: String,
    path: String,
    remote: String,
    branch: String,
    upstream_remote: String,
    upstream_branch: String,
    depth: i64,
    #[serde(skip)]
    status: Option<Rc<LazyStatus>>,
}

impl TreeEntry {
    fn new(view: &ViewConfig, depth: i64, status: Option<&Rc<StatusContext>>) -> Self {
        Self {
            name: view.name.clone(),
            path: view.path.clone(),
            remote: view.remote.clone(),
            branch: view.branch.clone(),
            upstream_remote: view
                .upstream
                .as_ref()
                .map(|upstream| upstream.remote.clone())
                .unwrap_or_default(),
            upstream_branch: view
                .upstream
                .as_ref()
                .map(|upstream| upstream.branch.clone())
                .unwrap_or_default(),
            depth,
            status: status.map(|context| {
                Rc::new(LazyStatus {
                    context: context.clone(),
                    view: view.clone(),
                    cell: OnceCell::new(),
                })
            }),
        }
    }

    /// Where the view stands, or `None` when this repository cannot say.
    fn survey(&self) -> Option<TreeStatus> {
        self.status.as_ref().and_then(|status| status.get())
    }
}

/// The store every top-level view's status is read from, opened once.
struct StatusContext {
    git: Git,
    repo: RefCell<gix::Repository>,
    /// What the bookmark the views are derived from points at.
    anchor: gix::ObjectId,
    cache: RefCell<Cache>,
}

/// Publication-state inputs, or `None` when this repository cannot answer at
/// all -- no bookmark to derive the views from, most commonly. That is not an
/// error here: the tree is primarily about the manifests, and a repository
/// that cannot be compared simply draws without markers.
fn status_context(
    workspace_command: &WorkspaceCommandHelper,
    bookmark: &str,
) -> Option<Rc<StatusContext>> {
    // Whether this repository keeps Git objects at all is settled before the
    // bookmark is read as one, so a backend that keeps none draws its manifests
    // without markers instead of failing to render the tree.
    let (git, repo) = try_open_store(workspace_command).ok().flatten()?;
    let (_, target) = anchor(workspace_command, bookmark, "tree").ok()?;
    Some(Rc::new(StatusContext {
        git,
        repo: RefCell::new(repo),
        anchor: target,
        cache: RefCell::new(Cache::new()),
    }))
}

/// One view's comparison, computed at most once and only when a template
/// asks for a status keyword.
struct LazyStatus {
    context: Rc<StatusContext>,
    view: ViewConfig,
    cell: OnceCell<Option<TreeStatus>>,
}

impl LazyStatus {
    fn get(&self) -> Option<TreeStatus> {
        *self.cell.get_or_init(|| {
            let mut repo = self.context.repo.borrow_mut();
            let mut cache = self.context.cache.borrow_mut();
            // As of the last fetch, deliberately: a glance at the tree must
            // not reach the network. A view that was never fetched, or that
            // cannot be derived from the bookmark, answers `None` rather than
            // failing the whole tree; `jj views status` is where those
            // situations are reported with their reasons.
            survey(
                &self.context.git,
                &mut repo,
                &self.view,
                &self.context.anchor,
                false,
                Freshness::AsOfLastFetch,
                &mut cache,
            )
            .ok()
            .map(|survey| TreeStatus {
                ahead: clamp(survey.ahead),
                behind: clamp(survey.incoming.len()),
                elided: clamp(survey.elided),
                diverged: survey.position == Position::Diverged,
            })
        })
    }
}

/// Commit counts, as the template integer type.
#[derive(Clone, Copy)]
struct TreeStatus {
    ahead: i64,
    behind: i64,
    elided: i64,
    diverged: bool,
}

fn clamp(count: usize) -> i64 {
    i64::try_from(count).unwrap_or(i64::MAX)
}

type ViewsTreeTemplateLanguage = GenericTemplateLanguage<'static, TreeEntry>;

generic_templater::impl_self_property_wrapper!(TreeEntry);

// TreeEntry is cloned internally in the templater; the shared status cell
// rides along in an Rc so every clone answers from one computation.
fn views_tree_template_language(
    settings: &UserSettings,
    current_dir: &Path,
) -> ViewsTreeTemplateLanguage {
    let mut language = ViewsTreeTemplateLanguage::new(settings, current_dir);
    language.add_keyword("name", |self_property| {
        let out_property = self_property.map(|entry| entry.name);
        Ok(out_property.into_dyn_wrapped())
    });
    language.add_keyword("path", |self_property| {
        let out_property = self_property.map(|entry| entry.path);
        Ok(out_property.into_dyn_wrapped())
    });
    language.add_keyword("remote", |self_property| {
        let out_property = self_property.map(|entry| entry.remote);
        Ok(out_property.into_dyn_wrapped())
    });
    language.add_keyword("branch", |self_property| {
        let out_property = self_property.map(|entry| entry.branch);
        Ok(out_property.into_dyn_wrapped())
    });
    language.add_keyword("upstream_remote", |self_property| {
        let out_property = self_property.map(|entry| entry.upstream_remote);
        Ok(out_property.into_dyn_wrapped())
    });
    language.add_keyword("upstream_branch", |self_property| {
        let out_property = self_property.map(|entry| entry.upstream_branch);
        Ok(out_property.into_dyn_wrapped())
    });
    language.add_keyword("depth", |self_property| {
        let out_property = self_property.map(|entry| entry.depth);
        Ok(out_property.into_dyn_wrapped())
    });
    language.add_keyword("tracked", |self_property| {
        let out_property = self_property.map(|entry| entry.survey().is_some());
        Ok(out_property.into_dyn_wrapped())
    });
    language.add_keyword("ahead", |self_property| {
        let out_property = self_property.map(|entry| entry.survey().map(|status| status.ahead));
        Ok(out_property.into_dyn_wrapped())
    });
    language.add_keyword("behind", |self_property| {
        let out_property = self_property.map(|entry| entry.survey().map(|status| status.behind));
        Ok(out_property.into_dyn_wrapped())
    });
    language.add_keyword("elided", |self_property| {
        let out_property = self_property.map(|entry| entry.survey().map(|status| status.elided));
        Ok(out_property.into_dyn_wrapped())
    });
    language.add_keyword("diverged", |self_property| {
        let out_property =
            self_property.map(|entry| entry.survey().is_some_and(|status| status.diverged));
        Ok(out_property.into_dyn_wrapped())
    });
    language
}
