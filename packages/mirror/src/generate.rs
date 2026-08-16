//! `mirror gen`: materialize one package as a self-contained source tree. The
//! primary crate sits at the output root; its intra-workspace dependency
//! closure (from the root manifest's `[workspace.dependencies]` path entries)
//! goes under `crates/<name>/`, stitched together by an emitted `[workspace]`
//! when the closure is non-empty. The pruned `Cargo.lock`, pinned toolchain,
//! and root LICENSE ride along so the tree builds exactly like the monorepo.

use std::collections::BTreeMap;
use std::fs;
use std::path::Path;

use anyhow::{Context, Result, bail};

use crate::workspace::Workspace;
use crate::{MONOREPO_SLUG, changelog, lockfile, manifest, readme};

pub struct Request<'a> {
    /// Repo-relative package path, e.g. `packages/progress-style`.
    pub package: &'a Path,
    pub out: &'a Path,
    pub mirror_repo: Option<&'a str>,
    /// Pitch for the generated README (the resolved `mirror.description`);
    /// `None` falls back to the crate's `[package] description`.
    pub description: Option<&'a str>,
    /// Monorepo flake output attr when the package is flake-exposed.
    pub flake_attr: Option<&'a str>,
}

pub struct Generated {
    pub crate_name: String,
    /// Internal dependency crate names placed under `crates/`, sorted.
    pub internal: Vec<String>,
}

/// Files at the top of each copied crate that only make sense inside the
/// monorepo's nix machinery, plus build artifacts.
const SKIP_TOP_LEVEL: [&str; 3] = ["default.nix", "package.nix", "target"];

pub fn run(workspace: &Workspace, request: &Request<'_>) -> Result<Generated> {
    let package_dir = workspace.root.join(request.package);
    let primary_manifest = read_manifest(&package_dir)?;
    let manifest::PackageInfo {
        name: crate_name,
        description,
    } = manifest::package_info(&primary_manifest)?;
    // The declarative mirror metadata is the single source of truth for the
    // pitch; the crate's own description is the fallback, never a second copy.
    let description = request.description.map(str::to_owned).or(description);
    let internal = dependency_closure(workspace, &primary_manifest)?;

    ensure_empty(request.out)?;
    copy_crate(&package_dir, request.out)?;
    for (name, path) in &internal {
        copy_crate(
            &workspace.root.join(path),
            &request.out.join("crates").join(name),
        )?;
    }

    let tables = manifest::WorkspaceTables {
        package: workspace.package_defaults()?,
        dependencies: workspace.dependencies()?,
    };
    let mut rewritten =
        manifest::standalone(&primary_manifest, &tables, &|name| format!("crates/{name}"))?;
    if !internal.is_empty() {
        let members: Vec<String> = internal
            .keys()
            .map(|name| format!("crates/{name}"))
            .collect();
        rewritten = manifest::append_workspace(&rewritten, &members)?;
    }
    fs::write(request.out.join("Cargo.toml"), rewritten).context("writing Cargo.toml")?;
    for (name, path) in &internal {
        let text = read_manifest(&workspace.root.join(path))?;
        let rewritten = manifest::standalone(&text, &tables, &|name| format!("../{name}"))?;
        fs::write(
            request.out.join("crates").join(name).join("Cargo.toml"),
            rewritten,
        )
        .with_context(|| format!("writing crates/{name}/Cargo.toml"))?;
    }

    let lock =
        fs::read_to_string(workspace.root.join("Cargo.lock")).context("reading Cargo.lock")?;
    let mut roots: Vec<&str> = vec![&crate_name];
    roots.extend(internal.keys().map(String::as_str));
    fs::write(
        request.out.join("Cargo.lock"),
        lockfile::prune(&lock, &roots)?,
    )
    .context("writing pruned Cargo.lock")?;

    for file in ["rust-toolchain.toml", "LICENSE"] {
        let source = workspace.root.join(file);
        if source.exists() {
            fs::copy(&source, request.out.join(file)).with_context(|| format!("copying {file}"))?;
        }
    }

    let package_path = request
        .package
        .to_str()
        .context("package path is not UTF-8")?
        .trim_end_matches('/');

    let history = workspace.package_history(package_path)?;
    if !history.is_empty() {
        fs::write(
            request.out.join("CHANGELOG.md"),
            changelog::compose(&changelog::Request {
                monorepo: MONOREPO_SLUG,
                package_path,
                crate_name: &crate_name,
                history: &history,
            }),
        )
        .context("writing CHANGELOG.md")?;
    }

    // The banner sha rides inside the mirrored tree, so it has to be a
    // function of the package alone. Taking it from the monorepo HEAD made
    // every sync run rewrite the README and commit it to every mirror,
    // whatever the run's push actually changed (ENG-11556). A package with no
    // history yet has nothing better than HEAD to name.
    let source_commit = match history.first() {
        Some(change) => change.sha.clone(),
        None => workspace.head_commit()?,
    };

    let package = readme::Package {
        monorepo: MONOREPO_SLUG,
        path: package_path,
        source_commit: &source_commit,
        crate_name: &crate_name,
        description: description.as_deref(),
        mirror_repo: request.mirror_repo,
        flake_attr: request.flake_attr,
        has_binary: has_binary(&package_dir, &primary_manifest)?,
        has_changelog: !history.is_empty(),
    };
    write_readme(request.out, &package_dir, &package)?;

    Ok(Generated {
        crate_name,
        internal: internal.into_keys().collect(),
    })
}

/// BFS the intra-workspace dependency closure: crate name -> repo-relative
/// path, for every workspace path dependency reachable from `primary`.
fn dependency_closure(workspace: &Workspace, primary: &str) -> Result<BTreeMap<String, String>> {
    let mut closure = BTreeMap::new();
    let mut queue = vec![primary.to_owned()];
    while let Some(text) = queue.pop() {
        for name in manifest::inherited_dependency_names(&text)? {
            let Some(path) = workspace.dependency_path(&name)? else {
                continue;
            };
            if closure.insert(name, path.to_owned()).is_none() {
                queue.push(read_manifest(&workspace.root.join(path))?);
            }
        }
    }
    Ok(closure)
}

/// One hero variant's renderer: (crate name, tagline) to SVG text.
type HeroRender = fn(&str, Option<&str>) -> String;

/// Compose the mirror README, synthesizing `assets/hero.svg` and its
/// `assets/hero-dark.svg` twin first when the package ships no README of
/// its own; a curated README references its own pair (already copied with
/// the crate) per the creating-a-readme skill.
fn write_readme(out: &Path, package_dir: &Path, package: &readme::Package<'_>) -> Result<()> {
    let existing = fs::read_to_string(package_dir.join("README.md")).ok();
    if existing.is_none() {
        let heroes: [(&str, HeroRender); 2] = [
            (readme::HERO_PATH, readme::hero_svg),
            (readme::HERO_DARK_PATH, readme::hero_dark_svg),
        ];
        for (rel, render) in heroes {
            let hero = out.join(rel);
            if hero.exists() {
                continue;
            }
            fs::create_dir_all(hero.parent().context("hero path has a parent")?)
                .context("creating the hero's directory")?;
            fs::write(&hero, render(package.crate_name, package.description))
                .with_context(|| format!("writing {rel}"))?;
        }
    }
    fs::write(
        out.join("README.md"),
        readme::compose(package, existing.as_deref()),
    )
    .context("writing README.md")
}

/// Whether the crate builds an executable: `src/main.rs`, a `src/bin/`
/// directory (cargo's auto-discovered targets), or an explicit `[[bin]]`.
fn has_binary(package_dir: &Path, manifest: &str) -> Result<bool> {
    if package_dir.join("src/main.rs").is_file() || package_dir.join("src/bin").is_dir() {
        return Ok(true);
    }
    manifest::declares_binary(manifest)
}

fn read_manifest(crate_dir: &Path) -> Result<String> {
    let path = crate_dir.join("Cargo.toml");
    fs::read_to_string(&path).with_context(|| format!("reading {}", path.display()))
}

fn ensure_empty(out: &Path) -> Result<()> {
    if out.exists() && fs::read_dir(out).context("reading --out")?.next().is_some() {
        bail!("output directory {} is not empty", out.display());
    }
    fs::create_dir_all(out).with_context(|| format!("creating {}", out.display()))
}

fn copy_crate(source: &Path, target: &Path) -> Result<()> {
    fs::create_dir_all(target).with_context(|| format!("creating {}", target.display()))?;
    for entry in fs::read_dir(source).with_context(|| format!("reading {}", source.display()))? {
        let entry = entry?;
        let name = entry.file_name();
        if SKIP_TOP_LEVEL.iter().any(|skip| name == *skip) {
            continue;
        }
        copy_recursively(&entry.path(), &target.join(&name))?;
    }
    Ok(())
}

pub fn copy_recursively(source: &Path, target: &Path) -> Result<()> {
    if source.is_dir() {
        fs::create_dir_all(target).with_context(|| format!("creating {}", target.display()))?;
        for entry in
            fs::read_dir(source).with_context(|| format!("reading {}", source.display()))?
        {
            let entry = entry?;
            copy_recursively(&entry.path(), &target.join(entry.file_name()))?;
        }
    } else {
        fs::copy(source, target)
            .with_context(|| format!("copying {} -> {}", source.display(), target.display()))?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::collections::{BTreeMap, BTreeSet};
    use std::fmt::Write as _;
    use std::path::PathBuf;

    use super::*;
    use crate::exec;

    const ROOT_MANIFEST: &str = "\
[workspace]
members = [\"packages/example\"]

[workspace.package]
version = \"0.1.0\"
edition = \"2024\"

[workspace.dependencies]
";

    const MEMBER_MANIFEST: &str = "\
[package]
name = \"example\"
version.workspace = true
edition.workspace = true
description = \"An example crate: nothing but a marker.\"
";

    const LOCK: &str = "\
# This file is automatically @generated by Cargo.
# It is not intended for manual editing.
version = 4

[[package]]
name = \"example\"
version = \"0.1.0\"
";

    /// A scratch monorepo under git: workspace root, one member crate, and a
    /// lock naming it, committed so `package_history` has something to read.
    struct Scratch {
        /// Owns the on-disk tree for the duration of the test, and roots the
        /// generator's output directories.
        dir: tempfile::TempDir,
        workspace: Workspace,
    }

    fn scratch_monorepo() -> Scratch {
        let dir = tempfile::tempdir().expect("tempdir");
        let root = dir.path();
        fs::write(root.join("Cargo.toml"), ROOT_MANIFEST).expect("root manifest");
        fs::write(root.join("Cargo.lock"), LOCK).expect("lock");
        let package = root.join("packages/example");
        fs::create_dir_all(package.join("src")).expect("package dir");
        fs::write(package.join("Cargo.toml"), MEMBER_MANIFEST).expect("member manifest");
        fs::write(package.join("src/lib.rs"), "pub fn example() {}\n").expect("lib.rs");
        exec::git(root, &["init", "--quiet"]).expect("git init");
        // A signing key or an autocrlf inherited from the developer's global
        // config would break the scratch commits.
        for (key, value) in [("commit.gpgsign", "false"), ("core.autocrlf", "false")] {
            exec::git(root, &["config", key, value]).expect("git config");
        }
        commit(root, "feat: add the example crate");
        let workspace = Workspace::locate(Some(root)).expect("workspace");
        Scratch { dir, workspace }
    }

    fn commit(root: &Path, message: &str) {
        exec::git(root, &["add", "-A"]).expect("git add");
        exec::git(
            root,
            &[
                "-c",
                "user.name=mirror-test",
                "-c",
                "user.email=mirror-test@example.invalid",
                "commit",
                "--quiet",
                "--allow-empty",
                "-m",
                message,
            ],
        )
        .expect("git commit");
    }

    fn generate_into(workspace: &Workspace, out: &Path) {
        run(
            workspace,
            &Request {
                package: Path::new("packages/example"),
                out,
                mirror_repo: Some("owner/example"),
                description: None,
                flake_attr: None,
            },
        )
        .expect("generates");
    }

    /// A generated tree as relative path -> bytes. Bytes, not text: a package
    /// may carry binary assets, and the claim under test is byte identity.
    type Tree = BTreeMap<PathBuf, Vec<u8>>;

    fn tree_contents(tree: &Path) -> Tree {
        let mut files = Tree::new();
        collect(tree, tree, &mut files);
        files
    }

    fn collect(root: &Path, dir: &Path, files: &mut Tree) {
        for entry in fs::read_dir(dir).expect("reading generated tree") {
            let path = entry.expect("entry").path();
            if path.is_dir() {
                collect(root, &path, files);
            } else {
                let relative = path.strip_prefix(root).expect("under root").to_path_buf();
                files.insert(relative, fs::read(&path).expect("reading generated file"));
            }
        }
    }

    /// Every path the two trees disagree on, rendered for an assertion
    /// message: a plain `assert_eq!` on the byte maps prints kilobytes of
    /// integer array and hides which file actually moved.
    fn differences(before: &Tree, after: &Tree) -> String {
        let paths: BTreeSet<&PathBuf> = before.keys().chain(after.keys()).collect();
        let mut out = String::new();
        for path in paths {
            let (left, right) = (before.get(path), after.get(path));
            if left == right {
                continue;
            }
            let render = |bytes: Option<&Vec<u8>>| {
                bytes.map_or_else(
                    || "<absent>".to_owned(),
                    |bytes| String::from_utf8_lossy(bytes).into_owned(),
                )
            };
            let _ = write!(
                out,
                "{}:\n--- before\n{}\n+++ after\n{}\n",
                path.display(),
                render(left),
                render(right)
            );
        }
        out
    }

    /// The generated tree must be a function of the package, not of when the
    /// generator ran: `publish` commits whenever the tree differs from the
    /// mirror's, so a tree that moves with the monorepo HEAD puts a one-line
    /// README diff on every mirror on every sync run (ENG-11556).
    #[test]
    fn a_monorepo_commit_elsewhere_leaves_the_tree_byte_identical() {
        let scratch = scratch_monorepo();
        let before = scratch.dir.path().join("before");
        generate_into(&scratch.workspace, &before);
        let head_before = scratch.workspace.head_commit().expect("HEAD");

        commit(
            &scratch.workspace.root,
            "docs: a commit that touches no package",
        );
        let head_after = scratch.workspace.head_commit().expect("HEAD");
        assert_ne!(head_before, head_after, "the commit must move HEAD");

        let after = scratch.dir.path().join("after");
        generate_into(&scratch.workspace, &after);

        let differences = differences(&tree_contents(&before), &tree_contents(&after));
        assert!(differences.is_empty(), "{differences}");
    }

    /// The banner still names a commit, and it is the one that produced the
    /// mirrored content. Without this the test above also passes for a banner
    /// that dropped the sha entirely.
    #[test]
    fn the_banner_names_the_packages_own_commit() {
        let scratch = scratch_monorepo();
        let package_commit = scratch.workspace.head_commit().expect("HEAD");
        commit(
            &scratch.workspace.root,
            "docs: a commit that touches no package",
        );
        let head = scratch.workspace.head_commit().expect("HEAD");

        let out = scratch.dir.path().join("out");
        generate_into(&scratch.workspace, &out);
        let readme = fs::read_to_string(out.join("README.md")).expect("README.md");

        assert!(readme.contains(&package_commit), "{readme}");
        assert!(!readme.contains(&head), "{readme}");
    }
}
