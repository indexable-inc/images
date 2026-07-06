//! `mirror publish`: snapshot-sync a generated tree into its mirror repo. One
//! commit per effective change: the mirror's working tree is replaced with a
//! fresh `gen` output and committed only when the two differ, as
//! `sync: <monorepo>@<sha>` with a `Source-Commit` trailer. No history
//! filtering, no force-push; the mirror's history is the sequence of monorepo
//! states that actually changed the package.

use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use serde_json::Value;

use crate::workspace::Workspace;
use crate::{MONOREPO_SLUG, exec, generate, manifest};

pub struct Request {
    /// Repo-relative package path, e.g. `packages/progress-style`.
    pub package: PathBuf,
    pub remote_url: Option<String>,
    pub repo: Option<String>,
    pub create: bool,
    pub mirror_json: Option<PathBuf>,
}

struct Target {
    remote_url: String,
    repo: Option<String>,
    description: Option<String>,
    topics: Vec<String>,
}

pub fn run(workspace: &Workspace, request: &Request) -> Result<()> {
    let target = resolve_target(workspace, request)?;
    let scratch = tempfile::tempdir().context("creating scratch directory")?;

    let tree = scratch.path().join("tree");
    generate::run(
        workspace,
        &generate::Request {
            package: &request.package,
            out: &tree,
            mirror_repo: target.repo.as_deref(),
        },
    )?;

    let clone = scratch.path().join("repo");
    let url = authenticated_url(&target.remote_url);
    clone_or_create(&target, &url, scratch.path(), &clone, request.create)?;
    // Base `main` on the remote's when it exists (the clone may have checked
    // out a different or unborn default branch); a brand-new repo starts it.
    if exec::git(&clone, &["rev-parse", "--verify", "--quiet", "origin/main"]).is_ok() {
        exec::git(
            &clone,
            &["checkout", "--quiet", "-B", "main", "origin/main"],
        )?;
    } else {
        exec::git(&clone, &["checkout", "--quiet", "-B", "main"])?;
    }

    replace_worktree(&clone, &tree)?;
    exec::git(&clone, &["add", "-A"])?;
    if exec::git(&clone, &["status", "--porcelain"])?.is_empty() {
        println!("{}: mirror is up to date", request.package.display());
        return Ok(());
    }

    let sha = workspace.head_commit()?;
    exec::git(
        &clone,
        &[
            "-c",
            "user.name=index-mirror",
            "-c",
            "user.email=index-mirror@users.noreply.github.com",
            "commit",
            "--quiet",
            "-m",
            &format!("sync: {MONOREPO_SLUG}@{sha}"),
            "-m",
            &format!("Source-Commit: {sha}"),
        ],
    )?;
    exec::git(&clone, &["push", "origin", "main"])?;
    println!(
        "{}: pushed sync of {MONOREPO_SLUG}@{sha}",
        request.package.display()
    );
    Ok(())
}

/// Push coordinates for the package: an explicit `--remote-url`/`--repo`
/// wins, otherwise the entry for this package in the rendered
/// `.#lib.mirrorPackages` list (from `--mirror-json` or `nix eval`). A target
/// without a description falls back to the crate's own `[package]
/// description`, so packages needn't duplicate it in their `mirror` attr.
fn resolve_target(workspace: &Workspace, request: &Request) -> Result<Target> {
    let mut target = configured_target(workspace, request)?;
    if target.description.is_none() {
        target.description = crate_description(workspace, &request.package)?;
    }
    Ok(target)
}

fn configured_target(workspace: &Workspace, request: &Request) -> Result<Target> {
    if let Some(remote_url) = &request.remote_url {
        return Ok(Target {
            remote_url: remote_url.clone(),
            repo: request.repo.clone(),
            description: None,
            topics: Vec::new(),
        });
    }
    if let Some(repo) = &request.repo {
        return Ok(Target {
            remote_url: format!("https://github.com/{repo}.git"),
            repo: Some(repo.clone()),
            description: None,
            topics: Vec::new(),
        });
    }
    let entries = mirror_entries(workspace, request.mirror_json.as_deref())?;
    let package = request
        .package
        .to_str()
        .context("package path is not UTF-8")?;
    let entry = entries
        .iter()
        .find(|entry| entry.get("path").and_then(Value::as_str) == Some(package))
        .with_context(|| format!("`{package}` has no `mirror` attr in its package.nix"))?;
    let repo = entry
        .get("repo")
        .and_then(Value::as_str)
        .context("mirror entry without `repo`")?
        .to_owned();
    Ok(Target {
        remote_url: format!("https://github.com/{repo}.git"),
        repo: Some(repo),
        description: entry
            .get("description")
            .and_then(Value::as_str)
            .map(str::to_owned),
        topics: entry
            .get("topics")
            .and_then(Value::as_array)
            .map(|topics| {
                topics
                    .iter()
                    .filter_map(Value::as_str)
                    .map(str::to_owned)
                    .collect()
            })
            .unwrap_or_default(),
    })
}

/// The `[package] description` of the package's own `Cargo.toml`.
fn crate_description(workspace: &Workspace, package: &Path) -> Result<Option<String>> {
    let path = workspace.root.join(package).join("Cargo.toml");
    let text = fs::read_to_string(&path).with_context(|| format!("reading {}", path.display()))?;
    Ok(manifest::package_info(&text)?.description)
}

fn mirror_entries(workspace: &Workspace, json: Option<&Path>) -> Result<Vec<Value>> {
    let text = match json {
        Some(path) => {
            fs::read_to_string(path).with_context(|| format!("reading {}", path.display()))?
        }
        None => exec::run(
            &workspace.root,
            "nix",
            &["eval", "--json", ".#lib.mirrorPackages"],
        )?,
    };
    let value: Value = serde_json::from_str(&text).context("parsing mirrorPackages JSON")?;
    value
        .as_array()
        .cloned()
        .context("mirrorPackages JSON is not a list")
}

fn clone_or_create(
    target: &Target,
    url: &str,
    scratch: &Path,
    clone: &Path,
    create: bool,
) -> Result<()> {
    let clone_arg = clone.to_str().context("scratch path is not UTF-8")?;
    if exec::git(scratch, &["clone", "--quiet", url, clone_arg]).is_ok() {
        return Ok(());
    }
    if !create {
        bail!(
            "cloning {} failed; pass --create to create the mirror repo",
            target.remote_url
        );
    }
    let repo = target
        .repo
        .as_deref()
        .context("--create needs an `owner/name` repo, not just --remote-url")?;
    let mut args = vec!["repo", "create", repo, "--public", "--disable-wiki"];
    if let Some(description) = &target.description {
        args.extend(["--description", description]);
    }
    exec::run(scratch, "gh", &args)?;
    for topic in &target.topics {
        exec::run(scratch, "gh", &["repo", "edit", repo, "--add-topic", topic])?;
    }
    fs::create_dir_all(clone).context("creating clone directory")?;
    exec::git(clone, &["init", "--quiet"])?;
    exec::git(clone, &["remote", "add", "origin", url])?;
    Ok(())
}

/// Swap the clone's working tree for the generated one, leaving `.git` alone.
fn replace_worktree(clone: &Path, tree: &Path) -> Result<()> {
    for entry in fs::read_dir(clone).context("reading clone")? {
        let entry = entry?;
        if entry.file_name() == ".git" {
            continue;
        }
        let path = entry.path();
        if path.is_dir() {
            fs::remove_dir_all(&path).with_context(|| format!("removing {}", path.display()))?;
        } else {
            fs::remove_file(&path).with_context(|| format!("removing {}", path.display()))?;
        }
    }
    for entry in fs::read_dir(tree).context("reading generated tree")? {
        let entry = entry?;
        generate::copy_recursively(&entry.path(), &clone.join(entry.file_name()))?;
    }
    Ok(())
}

/// Embed `MIRROR_TOKEN` into a GitHub https URL so CI can push; error paths
/// redact the token (see `exec::redact`).
pub fn authenticated_url(url: &str) -> String {
    let token = std::env::var("MIRROR_TOKEN").unwrap_or_default();
    if token.is_empty() {
        return url.to_owned();
    }
    url.strip_prefix("https://github.com/").map_or_else(
        || url.to_owned(),
        |rest| format!("https://x-access-token:{token}@github.com/{rest}"),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A scratch monorepo: a workspace root, one member crate, and a rendered
    /// mirror manifest naming that crate.
    struct Scratch {
        /// Owns the on-disk tree for the duration of the test.
        _dir: tempfile::TempDir,
        workspace: Workspace,
        request: Request,
    }

    fn scratch(member_manifest: &str, mirror_entries: &str) -> Scratch {
        let dir = tempfile::tempdir().expect("tempdir");
        fs::write(dir.path().join("Cargo.toml"), "[workspace]\n").expect("root manifest");
        let package = dir.path().join("packages").join("example");
        fs::create_dir_all(&package).expect("package dir");
        fs::write(package.join("Cargo.toml"), member_manifest).expect("member manifest");
        let mirror_json = dir.path().join("mirror.json");
        fs::write(&mirror_json, mirror_entries).expect("mirror manifest");
        let workspace = Workspace::locate(Some(dir.path())).expect("workspace");
        Scratch {
            _dir: dir,
            workspace,
            request: Request {
                package: PathBuf::from("packages/example"),
                remote_url: None,
                repo: None,
                create: true,
                mirror_json: Some(mirror_json),
            },
        }
    }

    #[test]
    fn mirror_entry_description_wins_over_crate_manifest() {
        let scratch = scratch(
            "[package]\nname = \"example\"\ndescription = \"from Cargo.toml\"\n",
            r#"[{"path": "packages/example", "repo": "owner/example", "description": "from package.nix"}]"#,
        );
        let target = resolve_target(&scratch.workspace, &scratch.request).expect("resolves");
        assert_eq!(target.description.as_deref(), Some("from package.nix"));
    }

    #[test]
    fn missing_mirror_description_falls_back_to_crate_manifest() {
        let scratch = scratch(
            "[package]\nname = \"example\"\ndescription = \"from Cargo.toml\"\n",
            r#"[{"path": "packages/example", "repo": "owner/example"}]"#,
        );
        let target = resolve_target(&scratch.workspace, &scratch.request).expect("resolves");
        assert_eq!(target.description.as_deref(), Some("from Cargo.toml"));
    }
}
