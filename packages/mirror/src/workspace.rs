//! The monorepo workspace: root location plus the `[workspace.package]`
//! defaults and `[workspace.dependencies]` table that member manifests
//! inherit from.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use toml_edit::{DocumentMut, Item, Table};

use crate::exec;

pub struct Workspace {
    pub root: PathBuf,
    doc: DocumentMut,
}

impl Workspace {
    /// Load the workspace rooted at `root`, or when `None`, at the nearest
    /// ancestor of the current directory whose `Cargo.toml` has a
    /// `[workspace]` table.
    pub fn locate(root: Option<&Path>) -> Result<Self> {
        let root = match root {
            Some(dir) => dir.to_path_buf(),
            None => find_root(&std::env::current_dir()?)?,
        };
        let manifest_path = root.join("Cargo.toml");
        let text = std::fs::read_to_string(&manifest_path)
            .with_context(|| format!("reading {}", manifest_path.display()))?;
        let doc: DocumentMut = text
            .parse()
            .with_context(|| format!("parsing {}", manifest_path.display()))?;
        if doc.get("workspace").is_none() {
            bail!("{} has no [workspace] table", manifest_path.display());
        }
        Ok(Self { root, doc })
    }

    pub fn package_defaults(&self) -> Result<&Table> {
        self.workspace_table("package")
    }

    pub fn dependencies(&self) -> Result<&Table> {
        self.workspace_table("dependencies")
    }

    /// Repo-relative path of workspace dependency `name`, when that entry is
    /// an intra-workspace path dependency.
    pub fn dependency_path(&self, name: &str) -> Result<Option<&str>> {
        let Some(entry) = self.dependencies()?.get(name) else {
            return Ok(None);
        };
        Ok(entry
            .as_table_like()
            .and_then(|table| table.get("path"))
            .and_then(Item::as_str))
    }

    pub fn head_commit(&self) -> Result<String> {
        exec::git(&self.root, &["rev-parse", "HEAD"])
    }

    fn workspace_table(&self, name: &str) -> Result<&Table> {
        self.doc["workspace"]
            .get(name)
            .and_then(Item::as_table)
            .with_context(|| format!("root Cargo.toml has no [workspace.{name}] table"))
    }
}

fn find_root(start: &Path) -> Result<PathBuf> {
    for dir in start.ancestors() {
        let manifest = dir.join("Cargo.toml");
        if !manifest.exists() {
            continue;
        }
        let text = std::fs::read_to_string(&manifest)
            .with_context(|| format!("reading {}", manifest.display()))?;
        let has_workspace = text
            .parse::<DocumentMut>()
            .is_ok_and(|doc| doc.get("workspace").is_some());
        if has_workspace {
            return Ok(dir.to_path_buf());
        }
    }
    bail!(
        "no workspace Cargo.toml found above {} (pass --root)",
        start.display()
    );
}
