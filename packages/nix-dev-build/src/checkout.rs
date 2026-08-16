//! Finding a nix source checkout and reading which revision it holds.

use std::env;
use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};

/// The project name the nix tree's root `meson.build` declares. That file is a
/// stub whose only job is to pull `src/*` in as meson subprojects, which is the
/// arrangement ninja needs to build the tree at all, so finding this name is
/// what separates a nix checkout from any other meson project.
const ROOT_PROJECT: &str = "nix-dev-shell";

/// A validated nix source checkout.
pub struct Checkout {
    pub root: PathBuf,
}

/// What `git` says about the checked-out tree. `dirty` covers tracked files
/// only: the meson build directory is untracked, so counting untracked files
/// would report every checkout as dirty once it had been built once.
pub struct Revision {
    pub short: String,
    pub dirty: bool,
}

impl Checkout {
    /// Take `explicit` if given, otherwise search upward from the working
    /// directory. Either way the result is validated, so a wrong directory is
    /// refused here rather than surfacing later as a confusing meson error.
    pub fn find(explicit: Option<PathBuf>) -> Result<Self> {
        let root = match explicit {
            Some(path) => path
                .canonicalize()
                .with_context(|| format!("resolving --checkout {}", path.display()))?,
            None => search_upward()?,
        };
        validate(&root)?;
        Ok(Self { root })
    }

    /// `None` when the checkout is not a git working tree, which is legitimate:
    /// an unpacked tarball still builds.
    pub fn revision(&self) -> Option<Revision> {
        let short = git_probe::output(&self.root, &["rev-parse", "--short", "HEAD"])?;
        // --untracked-files=no keeps this from walking the build directory,
        // which holds tens of thousands of untracked object files.
        let status = git_probe::output(
            &self.root,
            &["status", "--porcelain", "--untracked-files=no"],
        )?;
        Some(Revision {
            short,
            dirty: !status.is_empty(),
        })
    }
}

fn search_upward() -> Result<PathBuf> {
    let start = env::current_dir().context("reading the working directory")?;
    for candidate in start.ancestors() {
        if is_nix_tree(candidate) {
            return Ok(candidate.to_path_buf());
        }
    }
    bail!(
        "no nix source checkout at or above {}\n  \
         pass --checkout <path>, or run this from inside a checkout",
        start.display()
    );
}

/// The cheap form of [`validate`], used while searching so that a directory
/// which merely fails the check is skipped instead of aborting the search.
fn is_nix_tree(root: &Path) -> bool {
    declares_root_project(root).unwrap_or(false) && root.join("src/libexpr/meson.build").is_file()
}

/// Refuse anything that is not a nix source checkout, naming each thing that
/// was looked for and whether it was there. The failure this catches is being
/// pointed at the wrong tree, and the useful error says which half was wrong.
fn validate(root: &Path) -> Result<()> {
    let root_meson = declares_root_project(root)?;
    let libexpr = root.join("src/libexpr/meson.build").is_file();
    let flake = root.join("flake.nix").is_file();
    if root_meson && libexpr && flake {
        return Ok(());
    }
    bail!(
        "{} is not a nix source checkout\n  \
         meson.build declaring project('{ROOT_PROJECT}'): {}\n  \
         src/libexpr/meson.build: {}\n  \
         flake.nix (needed for the dev shell): {}",
        root.display(),
        found(root_meson),
        found(libexpr),
        found(flake),
    );
}

const fn found(present: bool) -> &'static str {
    if present { "found" } else { "missing" }
}

/// Whether the root `meson.build` declares the nix stub project. A missing file
/// is a plain `false`; an unreadable one is an error, because silently treating
/// a permissions problem as "wrong directory" sends the reader to the wrong
/// question.
fn declares_root_project(root: &Path) -> Result<bool> {
    let path = root.join("meson.build");
    match fs::read_to_string(&path) {
        Ok(text) => Ok(text.contains(&format!("'{ROOT_PROJECT}'"))),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(error) => Err(error).with_context(|| format!("reading {}", path.display())),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn nix_tree(root: &Path) {
        fs::create_dir_all(root.join("src/libexpr")).unwrap();
        fs::write(
            root.join("meson.build"),
            format!("project(\n  '{ROOT_PROJECT}',\n  'cpp',\n)\n"),
        )
        .unwrap();
        fs::write(root.join("src/libexpr/meson.build"), "").unwrap();
        fs::write(root.join("flake.nix"), "{}").unwrap();
    }

    #[test]
    fn accepts_a_nix_tree() {
        let dir = tempfile::tempdir().unwrap();
        nix_tree(dir.path());
        validate(dir.path()).unwrap();
    }

    #[test]
    fn names_each_missing_marker() {
        let dir = tempfile::tempdir().unwrap();
        // A meson project that is not nix: the root file is present and parses,
        // so only the project name distinguishes it.
        fs::write(dir.path().join("meson.build"), "project('something-else')").unwrap();
        let error = validate(dir.path()).unwrap_err().to_string();
        assert!(error.contains("is not a nix source checkout"), "{error}");
        assert!(
            error.contains(&format!("project('{ROOT_PROJECT}'): missing")),
            "{error}"
        );
        assert!(
            error.contains("src/libexpr/meson.build: missing"),
            "{error}"
        );
    }

    #[test]
    fn a_subdirectory_resolves_to_the_checkout_root() {
        let dir = tempfile::tempdir().unwrap();
        // The temporary directory itself may be a symlink (/var on darwin), and
        // ancestors() does not resolve those, so compare canonical paths.
        let root = dir.path().canonicalize().unwrap();
        nix_tree(&root);
        let deep = root.join("src/libexpr");
        let hit = deep
            .ancestors()
            .find(|candidate| is_nix_tree(candidate))
            .unwrap();
        assert_eq!(hit, root);
    }
}
