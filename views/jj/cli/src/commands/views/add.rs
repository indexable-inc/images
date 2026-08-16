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

use std::fmt::Write as _;
use std::io::Write as _;

use bstr::ByteSlice as _;
use clap_complete::ArgValueCandidates;
use jj_lib::backend::CommitId;
use jj_lib::backend::CopyId;
use jj_lib::backend::TreeId;
use jj_lib::backend::TreeValue;
use jj_lib::merge::Merge;
use jj_lib::merged_tree::MergedTree;
use jj_lib::merged_tree_builder::MergedTreeBuilder;
use jj_lib::object_id::ObjectId as _;
use jj_lib::repo::Repo as _;
use jj_lib::repo_path::RepoPathBuf;
use jj_views::Cache;
use jj_views::DeriveAnchor;
use tracing::instrument;

use super::READ_ONLY_UPSTREAM_REF_NAMESPACE;
use super::RawViewConfig;
use super::UPSTREAM_REF_NAMESPACE;
use super::VIEW_MANIFEST_FILE_NAME;
use super::ViewManifest;
use super::anchor;
use super::check_view_config;
use super::commit_tree;
use super::get_views_config;
use super::lift_error;
use super::open_store;
use super::resolve;
use super::validate_endpoints;
use crate::cli_util::CommandHelper;
use crate::command_error::CommandError;
use crate::command_error::internal_error;
use crate::command_error::user_error;
use crate::command_error::user_error_with_message;
use crate::complete;
use crate::ui::Ui;

/// Adopt an existing repository as a new view of this one
///
/// Ancestry first: the adopted tip is fetched, a host change is created whose
/// subtree at the path is that commit's tree exactly, and a child change
/// records the manifest entry whose anchor names the pair. The anchor only
/// ever points backward -- at its own parent and at an adopted commit that
/// predates both -- so nothing has to be published before the entry that
/// proves it is valid.
///
/// The host change is built from the adopted commit's tree object, never from
/// a working copy. A checkout-and-snapshot round trip silently loses a tracked
/// file that a `.gitignore` also names, and the loss is a view whose anchor
/// never validates.
///
/// Nothing is pushed. The view's remote is read to fetch the adopted tip and
/// to check for a branch that is already there; writing it stays
/// `jj views push`.
#[derive(clap::Args, Clone, Debug)]
pub struct ViewsAddArgs {
    /// Name for the view, its key in the manifest
    #[arg(value_name = "NAME")]
    name: String,

    /// Path prefix the adopted history will live under
    #[arg(long, value_name = "PATH")]
    path: String,

    /// URL the view is published to. Any URL `git push` accepts
    #[arg(long, value_name = "URL")]
    remote: String,

    /// The published repository's own default branch
    ///
    /// Also the branch the adopted tip is fetched from, unless
    /// --upstream-branch or --revision names something else.
    #[arg(long, default_value = "main", value_name = "NAME")]
    branch: String,

    /// Read-only upstream the view tracks, recorded as `upstream-remote`
    ///
    /// When given, the adopted tip is fetched from here rather than from
    /// --remote, which is the shape of adopting an upstream that a fork will
    /// be published for.
    #[arg(long, value_name = "URL", requires = "upstream_branch")]
    upstream_remote: Option<String>,

    /// Branch of the read-only upstream, recorded as `upstream-branch`
    #[arg(long, value_name = "NAME", requires = "upstream_remote")]
    upstream_branch: Option<String>,

    /// Full id of the commit to adopt, instead of the fetched branch's tip
    #[arg(long, value_name = "REVISION")]
    revision: Option<String>,

    /// Bookmark the two changes are created on top of
    ///
    /// The changes are not added to it; land them there before
    /// `jj views anchor` is asked to validate the entry.
    #[arg(long, short = 'b', default_value = "main", value_name = "NAME")]
    #[arg(add = ArgValueCandidates::new(complete::local_bookmarks))]
    bookmark: String,
}

#[instrument(skip_all)]
pub async fn cmd_views_add(
    ui: &mut Ui,
    command: &CommandHelper,
    args: &ViewsAddArgs,
) -> Result<(), CommandError> {
    let mut workspace_command = command.workspace_helper(ui).await?;
    let configured = get_views_config(&workspace_command).await?;

    // The name becomes a TOML table key written without quoting, so it is
    // held to bare-key characters rather than escaped: a manifest a person
    // appends to by hand should contain only entries a person would write.
    if args.name.is_empty()
        || !args
            .name
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-' || byte == b'_')
    {
        return Err(user_error(format!(
            "View name {:?} must use only letters, digits, `-` and `_`, so it can be a bare key \
             in {VIEW_MANIFEST_FILE_NAME}",
            args.name
        )));
    }
    let path = args.path.trim_matches('/');
    for view in &configured {
        if view.name == args.name {
            return Err(user_error(format!(
                "The {} view already exists, publishing {}",
                view.name, view.path
            ))
            .hinted(format!(
                "Pick another name, or `jj views fetch {}` to update the view that is there.",
                view.name
            )));
        }
        if view.path.trim_matches('/') == path {
            return Err(user_error(format!(
                "The {} view already publishes {}",
                view.name, view.path
            ))
            .hinted("Pick another path; two views of one prefix would derive the same history."));
        }
    }

    // Checked exactly the way every later command will read the entry, so a
    // value this command accepts is one the manifest can hold.
    let view = check_view_config(
        args.name.clone(),
        RawViewConfig {
            path: path.to_owned(),
            remote: args.remote.clone(),
            branch: args.branch.clone(),
            anchor: None,
            upstream_remote: args.upstream_remote.clone(),
            upstream_branch: args.upstream_branch.clone(),
        },
    )?;
    let filter = view.filter()?;
    let entry = ManifestEntry::checked(&view)?;
    let revision = args
        .revision
        .as_deref()
        .map(|value| {
            if !matches!(value.len(), 40 | 64) {
                return Err(user_error(
                    "--revision must be a full 40 or 64 digit commit id; it becomes the manifest \
                     anchor's `view` value",
                ));
            }
            gix::ObjectId::from_hex(value.as_bytes())
                .map_err(|err| user_error(format!("--revision is not a commit id: {err}")))
        })
        .transpose()?;

    let (git, repo) = open_store(&workspace_command, "add")?;
    let (_, base) = anchor(&workspace_command, &args.bookmark, "add")?;
    validate_endpoints(&git, &[&view])?;

    let base_tree = commit_tree(&repo, base)?;
    if tree_path_occupied(&repo, base_tree, path)? {
        return Err(
            user_error(format!("{path} already exists in {}'s tree", args.bookmark)).hinted(
                "`jj views add` adopts a repository at a path that is not there yet. For a prefix \
                 that already carries the content, write the manifest entry by hand and let `jj \
                 views anchor` validate it.",
            ),
        );
    }

    // The manifest the child change appends to is the one in the bookmark's
    // tree: that is the file the change rewrites, whatever state the working
    // copy's checkout of it is in.
    let existing_manifest = manifest_in_tree(&repo, base_tree)?;
    if existing_manifest.is_none() && !configured.is_empty() {
        return Err(user_error(format!(
            "The views here are configured outside {VIEW_MANIFEST_FILE_NAME}"
        ))
        .hinted(format!(
            "`jj views add` appends to the manifest file, and creating one would shadow the \
             configured `[views]` tables. Move them into {VIEW_MANIFEST_FILE_NAME} on {} first.",
            args.bookmark
        )));
    }
    if let Some(manifest) = &existing_manifest {
        check_manifest_has_room(manifest, &args.name, path)?;
    }

    // Fetch the adopted tip: from the read-only upstream when one is given,
    // otherwise from the published remote itself. Depth 1 -- the anchor
    // deliberately cuts the adopted history off here, and `jj views anchor`
    // bootstraps fresh clones the same way.
    let (endpoint_remote, endpoint_branch, namespace) = match &view.upstream {
        Some(upstream) => (
            upstream.remote.as_str(),
            upstream.branch.as_str(),
            READ_ONLY_UPSTREAM_REF_NAMESPACE,
        ),
        None => (
            view.remote.as_str(),
            view.branch.as_str(),
            UPSTREAM_REF_NAMESPACE,
        ),
    };
    let selector = match revision {
        Some(id) => id.to_string(),
        None => format!("refs/heads/{endpoint_branch}"),
    };
    let tracking = format!("{namespace}{}", view.name);
    let repo = git
        .fetch_adopt_tip(endpoint_remote, &selector, &tracking)
        .map_err(|err| {
            user_error(format!(
                "Could not fetch {selector} from {endpoint_remote}: {err}"
            ))
            .hinted(
                "Nothing was recorded. Check --remote, --branch and --revision; `jj views add` \
                 fetches the adopted tip before it writes anything.",
            )
        })?;
    let adopted = resolve(&repo, &tracking)?
        .ok_or_else(|| internal_error(format!("the fetch did not write {tracking}")))?;

    check_published_branch(&git, &repo, &view, args, adopted)?;

    let adopted_tree = commit_tree(&repo, adopted)?;
    let host_tree = jj_views::graft_snapshot(&repo, &base_tree, &filter, &adopted_tree)
        .map_err(|err| lift_error(&view, err))?;

    let mut tx = workspace_command.start_transaction();
    let store = tx.repo().store().clone();
    let lift = tx
        .repo_mut()
        .new_commit(
            vec![CommitId::from_bytes(base.as_bytes())],
            MergedTree::resolved(store.clone(), TreeId::from_bytes(host_tree.as_bytes())),
        )
        .set_description(format!(
            "views add {}: adopt {adopted} at {path}\n",
            view.name
        ))
        .write()
        .await?;
    let lift_id = gix::ObjectId::from_bytes_or_panic(lift.id().as_bytes());

    // The whole point of the ordering: the anchor is written by a descendant
    // of the commit it names, so it can be checked the moment it exists.
    // Checked with the same comparison `jj views anchor` trusts entries by;
    // failing it here means this command built a lift it cannot vouch for,
    // and nothing should be finished.
    Cache::new()
        .seed_anchor_after_ancestry_check(
            &repo,
            &filter,
            DeriveAnchor {
                source: lift_id,
                view: adopted,
            },
        )
        .map_err(|err| lift_error(&view, err))?;

    let manifest = entry.appended_to(existing_manifest.as_deref(), lift_id, adopted);
    toml::from_str::<ViewManifest>(&manifest).map_err(|err| {
        internal_error(format!(
            "the updated {VIEW_MANIFEST_FILE_NAME} does not parse: {err}"
        ))
    })?;
    let manifest_path = RepoPathBuf::from_internal_string(VIEW_MANIFEST_FILE_NAME)
        .map_err(|err| internal_error(format!("bad manifest path: {err}")))?;
    let file_id = store
        .write_file(&manifest_path, &mut manifest.as_bytes())
        .await?;
    let mut manifest_tree = MergedTreeBuilder::new(MergedTree::resolved(
        store.clone(),
        TreeId::from_bytes(host_tree.as_bytes()),
    ));
    manifest_tree.set_or_remove(
        manifest_path,
        Merge::normal(TreeValue::File {
            id: file_id,
            executable: false,
            copy_id: CopyId::placeholder(),
        }),
    );
    let record = tx
        .repo_mut()
        .new_commit(vec![lift.id().clone()], manifest_tree.write_tree().await?)
        .set_description(format!(
            "views add {}: record the manifest entry and anchor\n",
            view.name
        ))
        .write()
        .await?;
    let record_id = gix::ObjectId::from_bytes_or_panic(record.id().as_bytes());
    tx.finish(ui, format!("views add {}", view.name)).await?;

    let mut out = ui.status();
    writeln!(
        out,
        "{}: adopted {adopted} from {endpoint_remote}",
        view.name
    )?;
    writeln!(out, "  {lift_id} carries its tree at {path}")?;
    writeln!(
        out,
        "  {record_id} records the manifest entry, anchor {lift_id} -> {adopted}"
    )?;
    writeln!(
        out,
        "Neither change is on {} yet. Land both there, then `jj views anchor {}` validates the \
         entry, local patches go on top as ordinary commits, and `jj views push {}` publishes \
         them.",
        args.bookmark, view.name, view.name
    )?;
    Ok(())
}

/// Fails when the published branch already exists with history the adopted
/// commit is not part of.
///
/// A branch like that -- typically an orphan left by an earlier vendoring that
/// never shared upstream's hashes -- makes the view unusable after the fact:
/// every command that positions the view walks ancestry from the anchor, and
/// `jj views push --replace-drifted` covers hash drift of derived content, not
/// unrelated history. Refusing now, with the remedy, beats a manifest entry
/// that fails everywhere later.
///
/// A branch the adopted commit is an ancestor of is fine: that is a fork that
/// has moved ahead, and `jj views fetch` is built for it.
fn check_published_branch(
    git: &super::Git,
    repo: &gix::Repository,
    view: &super::ViewConfig,
    args: &ViewsAddArgs,
    adopted: gix::ObjectId,
) -> Result<(), CommandError> {
    let published = git
        .remote_branch(&view.remote, &view.branch)
        .map_err(|err| {
            user_error(format!(
                "Could not ask {} about {}: {err}",
                view.remote, view.branch
            ))
        })?;
    let Some(published_tip) = published else {
        return Ok(());
    };
    if published_tip == adopted {
        return Ok(());
    }
    // The containment check needs the branch's history here. Full rather than
    // shallow, because the answer is about ancestry, and it lands in the same
    // tracking ref every fetch of this view will use.
    git.fetch(
        &view.remote,
        &view.branch,
        &format!("{UPSTREAM_REF_NAMESPACE}{}", view.name),
    )
    .map_err(|err| {
        user_error(format!(
            "Could not fetch the existing {} branch at {}: {err}",
            view.branch, view.remote
        ))
    })?;
    let reachable = git.reachable_commits(&published_tip).map_err(|err| {
        user_error(format!(
            "Could not read the history of the existing {} branch: {err}",
            view.branch
        ))
    })?;
    if reachable.contains(&adopted) {
        return Ok(());
    }
    let _ = repo;
    Err(user_error(format!(
        "{} already has a {} branch, at {published_tip}, and it does not descend from the adopted \
         commit {adopted}",
        view.remote, view.branch
    ))
    .hinted(format!(
        "`jj views push` could never extend that branch. If it is a fork of the same upstream, \
         adopt the commit it forked from: `jj views add {} --revision <that commit> ...`, then \
         `jj views fetch {}` brings the fork's own commits in. If it is orphaned vendoring that \
         never shared upstream's hashes, keep its tip on a pin ref, delete the branch at the \
         remote, and rerun.",
        args.name, args.name
    )))
}

/// Whether anything in `tree` sits at `path`, or blocks the way to it.
///
/// A blob partway down counts as much as an entry at the full path: grafting
/// would silently replace it, and a path someone can lose content under is a
/// path this command refuses.
fn tree_path_occupied(
    repo: &gix::Repository,
    tree: gix::ObjectId,
    path: &str,
) -> Result<bool, CommandError> {
    let mut current = tree;
    let mut components = path.split('/').peekable();
    while let Some(component) = components.next() {
        let raw = repo
            .find_object(current)
            .map_err(|err| user_error_with_message(format!("Could not read tree {current}"), err))?
            .detach()
            .data;
        let decoded = gix::objs::TreeRef::from_bytes(&raw, repo.object_hash()).map_err(|err| {
            user_error_with_message(format!("Could not parse tree {current}"), err)
        })?;
        let Some(entry) = decoded
            .entries
            .iter()
            .find(|entry| entry.filename == component.as_bytes().as_bstr())
        else {
            return Ok(false);
        };
        if components.peek().is_none() || !entry.mode.is_tree() {
            return Ok(true);
        }
        current = entry.oid.to_owned();
    }
    Ok(false)
}

/// The manifest blob in `tree`'s root, decoded.
fn manifest_in_tree(
    repo: &gix::Repository,
    tree: gix::ObjectId,
) -> Result<Option<String>, CommandError> {
    let raw = repo
        .find_object(tree)
        .map_err(|err| user_error_with_message(format!("Could not read tree {tree}"), err))?
        .detach()
        .data;
    let decoded = gix::objs::TreeRef::from_bytes(&raw, repo.object_hash())
        .map_err(|err| user_error_with_message(format!("Could not parse tree {tree}"), err))?;
    let Some(entry) = decoded
        .entries
        .iter()
        .find(|entry| entry.filename == VIEW_MANIFEST_FILE_NAME.as_bytes().as_bstr())
    else {
        return Ok(None);
    };
    let blob = repo
        .find_object(entry.oid.to_owned())
        .map_err(|err| {
            user_error_with_message(format!("Could not read {VIEW_MANIFEST_FILE_NAME}"), err)
        })?
        .detach()
        .data;
    String::from_utf8(blob)
        .map(Some)
        .map_err(|err| user_error(format!("{VIEW_MANIFEST_FILE_NAME} is not UTF-8: {err}")))
}

/// Refuses a manifest that already holds the name or the path.
///
/// [`get_views_config`] reads the working copy's manifest and the settings;
/// this reads the one being appended to, which can differ from both. A
/// duplicate name would not even parse back -- TOML rejects a repeated table
/// -- so it must be caught while the message can still say what to do.
fn check_manifest_has_room(manifest: &str, name: &str, path: &str) -> Result<(), CommandError> {
    let parsed: ViewManifest = toml::from_str(manifest).map_err(|err| {
        user_error_with_message(
            format!("Could not parse the committed {VIEW_MANIFEST_FILE_NAME}"),
            err,
        )
    })?;
    for (existing, raw) in &parsed.views {
        if existing == name {
            return Err(user_error(format!(
                "The {name} view already exists in the committed {VIEW_MANIFEST_FILE_NAME}"
            ))
            .hinted(format!(
                "Pick another name, or `jj views fetch {name}` to update the view that is there."
            )));
        }
        if raw.path.trim_matches('/') == path {
            return Err(user_error(format!(
                "The {existing} view already publishes {}",
                raw.path
            ))
            .hinted("Pick another path; two views of one prefix would derive the same history."));
        }
    }
    Ok(())
}

/// The strings of one manifest entry, checked to survive the trip into TOML
/// and back.
struct ManifestEntry {
    name: String,
    path: String,
    remote: String,
    branch: String,
    upstream: Option<(String, String)>,
}

impl ManifestEntry {
    fn checked(view: &super::ViewConfig) -> Result<Self, CommandError> {
        Ok(Self {
            name: view.name.clone(),
            path: toml_string("path", &view.path)?,
            remote: toml_string("remote", &view.remote)?,
            branch: toml_string("branch", &view.branch)?,
            upstream: view
                .upstream
                .as_ref()
                .map(|upstream| {
                    Ok::<_, CommandError>((
                        toml_string("upstream-remote", &upstream.remote)?,
                        toml_string("upstream-branch", &upstream.branch)?,
                    ))
                })
                .transpose()?,
        })
    }

    /// The manifest with this entry at the end.
    ///
    /// Existing content is preserved byte for byte: the manifest is the file
    /// most likely to be under someone's review comment or merge, and a
    /// reserialization that reorders or reflows it turns a one-entry addition
    /// into a whole-file diff.
    fn appended_to(
        &self,
        existing: Option<&str>,
        source: gix::ObjectId,
        adopted: gix::ObjectId,
    ) -> String {
        let mut out = existing.unwrap_or_default().to_owned();
        if !out.is_empty() {
            if !out.ends_with('\n') {
                out.push('\n');
            }
            if !out.ends_with("\n\n") {
                out.push('\n');
            }
        }
        let Self {
            name,
            path,
            remote,
            branch,
            upstream,
        } = self;
        write!(
            out,
            "[views.{name}]\npath = {path}\nremote = {remote}\nbranch = {branch}\n"
        )
        .expect("writing to a String cannot fail");
        if let Some((upstream_remote, upstream_branch)) = upstream {
            write!(
                out,
                "upstream-remote = {upstream_remote}\nupstream-branch = {upstream_branch}\n"
            )
            .expect("writing to a String cannot fail");
        }
        write!(
            out,
            "\n[views.{name}.anchor]\nsource = \"{source}\"\nview = \"{adopted}\"\n"
        )
        .expect("writing to a String cannot fail");
        out
    }
}

/// `value` as a TOML basic string.
///
/// Rust's `{:?}` writes the `\"` and `\\` escapes TOML defines, so it serves
/// as the serializer; a value it would escape any other way -- a control
/// character, or one of the rare code points `escape_debug` rewrites -- is
/// refused rather than written in a syntax TOML does not know.
fn toml_string(field: &str, value: &str) -> Result<String, CommandError> {
    let simple = value
        .chars()
        .all(|c| matches!(c, '"' | '\\') || c.escape_debug().count() == 1);
    if !simple {
        return Err(user_error(format!(
            "The {field} value {value:?} has a character that cannot be written into \
             {VIEW_MANIFEST_FILE_NAME}"
        )));
    }
    Ok(format!("{value:?}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(upstream: bool) -> ManifestEntry {
        ManifestEntry {
            name: "adoptee".to_owned(),
            path: "\"vendor/adoptee\"".to_owned(),
            remote: "\"https://example.invalid/adoptee.git\"".to_owned(),
            branch: "\"main\"".to_owned(),
            upstream: upstream.then(|| {
                (
                    "\"https://example.invalid/upstream.git\"".to_owned(),
                    "\"master\"".to_owned(),
                )
            }),
        }
    }

    fn ids() -> (gix::ObjectId, gix::ObjectId) {
        (
            gix::ObjectId::from_hex(b"1111111111111111111111111111111111111111").unwrap(),
            gix::ObjectId::from_hex(b"2222222222222222222222222222222222222222").unwrap(),
        )
    }

    #[test]
    fn an_entry_appends_after_exactly_one_blank_line_preserving_the_existing_bytes() {
        let existing = "[views.first]\npath = \"a\"\nremote = \"r\"\nbranch = \"b\"\n";
        let (source, adopted) = ids();
        let manifest = entry(false).appended_to(Some(existing), source, adopted);
        assert!(manifest.starts_with(existing), "existing bytes changed");
        assert_eq!(
            &manifest[existing.len()..],
            "\n[views.adoptee]\npath = \"vendor/adoptee\"\nremote = \
             \"https://example.invalid/adoptee.git\"\nbranch = \
             \"main\"\n\n[views.adoptee.anchor]\nsource = \
             \"1111111111111111111111111111111111111111\"\nview = \
             \"2222222222222222222222222222222222222222\"\n"
        );
    }

    #[test]
    fn an_upstream_endpoint_is_recorded_between_branch_and_anchor() {
        let (source, adopted) = ids();
        let manifest = entry(true).appended_to(None, source, adopted);
        assert!(
            manifest.contains(
                "branch = \"main\"\nupstream-remote = \
                 \"https://example.invalid/upstream.git\"\nupstream-branch = \
                 \"master\"\n\n[views.adoptee.anchor]"
            ),
            "unexpected layout: {manifest}"
        );
        toml::from_str::<ViewManifest>(&manifest).expect("the appended entry parses");
    }

    #[test]
    fn a_manifest_without_a_trailing_blank_line_still_separates_the_new_entry() {
        let existing = "[views.first]\npath = \"a\"\nremote = \"r\"\nbranch = \"b\"";
        let (source, adopted) = ids();
        let manifest = entry(false).appended_to(Some(existing), source, adopted);
        assert!(manifest.starts_with(existing));
        assert!(
            manifest[existing.len()..].starts_with("\n\n[views.adoptee]"),
            "missing separation: {manifest}"
        );
    }

    #[test]
    fn a_control_character_is_refused_rather_than_miswritten() {
        assert!(toml_string("remote", "a\u{7}b").is_err());
        assert_eq!(
            toml_string("remote", "C:\\repos\\adoptee").unwrap(),
            "\"C:\\\\repos\\\\adoptee\""
        );
    }
}
