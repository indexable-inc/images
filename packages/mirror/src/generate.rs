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

    let package = readme::Package {
        monorepo: MONOREPO_SLUG,
        path: package_path,
        commit: &workspace.head_commit()?,
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

/// Compose the mirror README, synthesizing an `assets/hero.svg` first when
/// the package ships no README of its own; a curated README references its
/// own hero (already copied with the crate) per the creating-a-readme skill.
fn write_readme(out: &Path, package_dir: &Path, package: &readme::Package<'_>) -> Result<()> {
    let existing = fs::read_to_string(package_dir.join("README.md")).ok();
    if existing.is_none() {
        let hero = out.join(readme::HERO_PATH);
        if !hero.exists() {
            fs::create_dir_all(hero.parent().context("hero path has a parent")?)
                .context("creating the hero's directory")?;
            fs::write(&hero, readme::hero_svg(package.crate_name, package.description))
                .context("writing the hero SVG")?;
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
