//! Rewrite a workspace member's `Cargo.toml` into standalone form: inline the
//! `[workspace.package]` / `[workspace.dependencies]` inheritance to concrete
//! values (cargo's own merge semantics, same as
//! lib/rust/replace-workspace-values.py), drop the `[lints]` table (the
//! workspace lint set names lints only the org's patched clippy knows), and
//! point intra-workspace dependencies at their location in the generated
//! tree. Format-preserving: comments and layout survive.

use anyhow::{Context, Result, bail};
use toml_edit::{Array, DocumentMut, InlineTable, Item, Table, TableLike, Value};

pub const DEP_SECTIONS: [&str; 3] = ["dependencies", "dev-dependencies", "build-dependencies"];

/// The two root-manifest tables member manifests inherit from.
pub struct WorkspaceTables<'a> {
    pub package: &'a Table,
    pub dependencies: &'a Table,
}

/// Rewrite `manifest` into standalone form. `internal_path` maps an
/// intra-workspace dependency name to the path the rewritten manifest should
/// reference (relative to the manifest's own directory).
pub fn standalone(
    manifest: &str,
    workspace: &WorkspaceTables<'_>,
    internal_path: &dyn Fn(&str) -> String,
) -> Result<String> {
    let mut doc: DocumentMut = manifest.parse().context("parsing member Cargo.toml")?;
    inline_package(&mut doc, workspace.package)?;
    doc.remove("lints");
    for section in DEP_SECTIONS {
        if let Some(table) = doc.get_mut(section).and_then(Item::as_table_like_mut) {
            inline_dependencies(table, workspace, internal_path)?;
        }
    }
    if let Some(targets) = doc.get_mut("target").and_then(Item::as_table_like_mut) {
        let cfgs: Vec<String> = targets.iter().map(|(cfg, _)| cfg.to_owned()).collect();
        for cfg in cfgs {
            let Some(target) = targets.get_mut(&cfg).and_then(Item::as_table_like_mut) else {
                continue;
            };
            for section in DEP_SECTIONS {
                if let Some(table) = target.get_mut(section).and_then(Item::as_table_like_mut) {
                    inline_dependencies(table, workspace, internal_path)?;
                }
            }
        }
    }
    Ok(doc.to_string())
}

/// Append the `[workspace]` table that turns a generated tree with internal
/// dependencies under `crates/` into a self-contained cargo workspace.
pub fn append_workspace(manifest: &str, members: &[String]) -> Result<String> {
    let mut doc: DocumentMut = manifest.parse().context("parsing rewritten Cargo.toml")?;
    let mut list = Array::new();
    for member in members {
        list.push(member.as_str());
    }
    let mut table = Table::new();
    table["members"] = toml_edit::value(list);
    table["resolver"] = toml_edit::value("3");
    doc["workspace"] = Item::Table(table);
    Ok(doc.to_string())
}

/// Every dependency name a manifest inherits from `[workspace.dependencies]`,
/// across all dependency sections including target-specific ones.
pub fn inherited_dependency_names(manifest: &str) -> Result<Vec<String>> {
    let doc: DocumentMut = manifest.parse().context("parsing member Cargo.toml")?;
    let mut names = Vec::new();
    let mut collect = |table: &dyn TableLike| {
        names.extend(
            table
                .iter()
                .filter(|(_, item)| inherits_workspace(item))
                .map(|(name, _)| name.to_owned()),
        );
    };
    for section in DEP_SECTIONS {
        if let Some(table) = doc.get(section).and_then(Item::as_table_like) {
            collect(table);
        }
    }
    if let Some(targets) = doc.get("target").and_then(Item::as_table_like) {
        for (_, target) in targets.iter() {
            let Some(target) = target.as_table_like() else {
                continue;
            };
            for section in DEP_SECTIONS {
                if let Some(table) = target.get(section).and_then(Item::as_table_like) {
                    collect(table);
                }
            }
        }
    }
    Ok(names)
}

/// The `name` and `description` of a manifest's `[package]`.
pub struct PackageInfo {
    pub name: String,
    pub description: Option<String>,
}

pub fn package_info(manifest: &str) -> Result<PackageInfo> {
    let doc: DocumentMut = manifest.parse().context("parsing member Cargo.toml")?;
    let package = doc
        .get("package")
        .and_then(Item::as_table_like)
        .context("Cargo.toml has no [package] table")?;
    let name = package
        .get("name")
        .and_then(Item::as_str)
        .context("[package] has no `name`")?
        .to_owned();
    let description = package
        .get("description")
        .and_then(Item::as_str)
        .map(str::to_owned);
    Ok(PackageInfo { name, description })
}

fn inline_package(doc: &mut DocumentMut, defaults: &Table) -> Result<()> {
    let package = doc
        .get_mut("package")
        .and_then(Item::as_table_like_mut)
        .context("member Cargo.toml has no [package] table")?;
    let keys: Vec<String> = package.iter().map(|(key, _)| key.to_owned()).collect();
    for key in keys {
        if !package.get(&key).is_some_and(inherits_workspace) {
            continue;
        }
        let value = defaults
            .get(&key)
            .and_then(Item::as_value)
            .with_context(|| format!("[workspace.package] has no `{key}` to inherit"))?;
        set(package, &key, Item::Value(plain(value)));
    }
    // The monorepo never publishes to crates.io and neither do its mirrors;
    // pin that decision explicitly since the workspace default is inlined away.
    set(package, "publish", Item::Value(Value::from(false)));
    if package.get("license").is_none() && package.get("license-file").is_none() {
        package.insert("license", Item::Value(Value::from("MIT")));
    }
    Ok(())
}

fn inline_dependencies(
    table: &mut dyn TableLike,
    workspace: &WorkspaceTables<'_>,
    internal_path: &dyn Fn(&str) -> String,
) -> Result<()> {
    let names: Vec<String> = table.iter().map(|(name, _)| name.to_owned()).collect();
    for name in names {
        let Some(item) = table.get(&name) else {
            continue;
        };
        if !inherits_workspace(item) {
            if item.as_table_like().is_some_and(|t| t.contains_key("path")) {
                bail!(
                    "dependency `{name}` uses a direct `path`; only workspace inheritance is supported"
                );
            }
            continue;
        }
        let local = item
            .as_table_like()
            .with_context(|| format!("dependency `{name}` is not a table"))?;
        let merged = inherit_dependency(&name, local, workspace.dependencies, internal_path)?;
        set(table, &name, Item::Value(merged));
    }
    Ok(())
}

/// Replace a key's value in place. `TableLike::insert` re-creates the key and
/// with it drops the key's decor — the comments above a dependency — so an
/// existing entry is assigned through `get_mut` instead.
fn set(table: &mut dyn TableLike, key: &str, value: Item) {
    match table.get_mut(key) {
        Some(item) => *item = value,
        None => {
            table.insert(key, value);
        }
    }
}

fn inherit_dependency(
    name: &str,
    local: &dyn TableLike,
    workspace_deps: &Table,
    internal_path: &dyn Fn(&str) -> String,
) -> Result<Value> {
    for (key, _) in local.iter() {
        if !matches!(
            key,
            "workspace"
                | "features"
                | "default-features"
                | "default_features"
                | "optional"
                | "package"
        ) {
            bail!("unsupported key `{key}` on inherited dependency `{name}`");
        }
    }
    let entry = workspace_deps.get(name).with_context(|| {
        format!(
            "`{name}` inherits from the workspace but [workspace.dependencies] has no such entry"
        )
    })?;
    let entry_table = entry.as_table_like();

    let mut out = InlineTable::new();
    if let Some(table) = entry_table {
        for (key, item) in table.iter() {
            if key == "features" {
                continue;
            }
            let value = item.as_value().with_context(|| {
                format!("workspace dependency `{name}`: `{key}` is not a value")
            })?;
            if key == "path" {
                out.insert("path", Value::from(internal_path(name)));
            } else {
                out.insert(key, plain(value));
            }
        }
    } else {
        let version = entry.as_str().with_context(|| {
            format!("workspace dependency `{name}` is neither a table nor a version string")
        })?;
        out.insert("version", Value::from(version));
    }

    let mut features = Array::new();
    extend_features(
        &mut features,
        entry_table.and_then(|t| t.get("features")),
        name,
    )?;
    extend_features(&mut features, local.get("features"), name)?;
    if !features.is_empty() {
        out.insert("features", Value::Array(features));
    }

    // A member may re-enable default features the workspace turned off; the
    // reverse (member `default-features = false` over an inheriting entry) is
    // ignored, matching cargo.
    let entry_default = entry_table
        .and_then(|t| get_bool(t, "default-features"))
        .unwrap_or(true);
    let local_default =
        get_bool(local, "default-features").or_else(|| get_bool(local, "default_features"));
    if local_default == Some(true) && !entry_default {
        out.insert("default-features", Value::from(true));
    }
    if get_bool(local, "optional") == Some(true) {
        out.insert("optional", Value::from(true));
    }
    if let Some(package) = local.get("package").and_then(Item::as_str) {
        out.insert("package", Value::from(package));
    }

    if out.len() == 1
        && let Some(version) = out.get("version").and_then(Value::as_str)
    {
        return Ok(Value::from(version));
    }
    out.fmt();
    Ok(Value::InlineTable(out))
}

fn extend_features(features: &mut Array, item: Option<&Item>, name: &str) -> Result<()> {
    let Some(item) = item else {
        return Ok(());
    };
    let list = item
        .as_array()
        .with_context(|| format!("dependency `{name}`: `features` is not an array"))?;
    for feature in list {
        let feature = feature
            .as_str()
            .with_context(|| format!("dependency `{name}`: non-string feature"))?;
        features.push(feature);
    }
    Ok(())
}

fn inherits_workspace(item: &Item) -> bool {
    item.as_table_like()
        .and_then(|table| get_bool(table, "workspace"))
        == Some(true)
}

fn get_bool(table: &dyn TableLike, key: &str) -> Option<bool> {
    table.get(key).and_then(Item::as_bool)
}

/// Clone a value with its surrounding whitespace/decor dropped, so a value
/// lifted out of the root manifest formats cleanly in its new home.
fn plain(value: &Value) -> Value {
    let mut value = value.clone();
    value.decor_mut().clear();
    value
}

#[cfg(test)]
mod tests {
    use super::*;

    // Derived from the real root manifest: the exact shapes the rewriter must
    // handle (plain version string, table with default-features, path dep).
    const ROOT: &str = r#"
[workspace]
members = ["packages/git-log-pretty"]

[workspace.package]
version = "0.1.0"
edition = "2024"
publish = false

[workspace.dependencies]
chrono = { version = "0.4", default-features = false }
git2 = { version = "0.21", default-features = false }
github-avatar = { path = "packages/github-avatar" }
indicatif = "0.18"
tempfile = "3"
"#;

    // Derived from packages/git-log-pretty/Cargo.toml: workspace-inherited
    // package keys, feature additions, a comment that must survive, and an
    // intra-workspace dependency.
    const MEMBER: &str = r#"[package]
name = "git-log-pretty"
version.workspace = true
edition.workspace = true
publish.workspace = true
description = "Pretty git log viewer"

[lints]
workspace = true

[dependencies]
chrono = { workspace = true, features = ["clock"] }
# vendored libgit2 keeps the build free of system deps.
git2 = { workspace = true, default-features = false, features = ["vendored-libgit2"] }
github-avatar.workspace = true
indicatif.workspace = true

[dev-dependencies]
tempfile.workspace = true
"#;

    fn rewrite(member: &str) -> String {
        let root: DocumentMut = ROOT.parse().expect("root fixture parses");
        let workspace = WorkspaceTables {
            package: root["workspace"]["package"]
                .as_table()
                .expect("package table"),
            dependencies: root["workspace"]["dependencies"]
                .as_table()
                .expect("dependencies table"),
        };
        standalone(member, &workspace, &|name| format!("crates/{name}")).expect("rewrite succeeds")
    }

    #[test]
    fn inlines_package_inheritance_and_pins_policy() {
        let out = rewrite(MEMBER);
        assert!(out.contains("version = \"0.1.0\""), "{out}");
        assert!(out.contains("edition = \"2024\""), "{out}");
        assert!(out.contains("publish = false"), "{out}");
        assert!(out.contains("license = \"MIT\""), "{out}");
        assert!(!out.contains("workspace"), "{out}");
    }

    #[test]
    fn drops_lints_table() {
        let out = rewrite(MEMBER);
        assert!(!out.contains("[lints]"), "{out}");
    }

    #[test]
    fn merges_dependency_features_and_keeps_comments() {
        let out = rewrite(MEMBER);
        assert!(
            out.contains(
                r#"chrono = { version = "0.4", default-features = false, features = ["clock"] }"#
            ),
            "{out}"
        );
        assert!(out.contains(r#"features = ["vendored-libgit2"]"#), "{out}");
        assert!(
            out.contains("# vendored libgit2 keeps the build free of system deps."),
            "{out}"
        );
        assert!(out.contains(r#"indicatif = "0.18""#), "{out}");
    }

    #[test]
    fn rewrites_internal_dependencies_to_generated_paths() {
        let out = rewrite(MEMBER);
        assert!(
            out.contains(r#"github-avatar = { path = "crates/github-avatar" }"#),
            "{out}"
        );
    }

    #[test]
    fn appends_workspace_table_for_internal_closure() {
        let out = rewrite(MEMBER);
        let out = append_workspace(&out, &["crates/github-avatar".to_owned()]).expect("appends");
        assert!(out.contains("[workspace]"), "{out}");
        assert!(
            out.contains(r#"members = ["crates/github-avatar"]"#),
            "{out}"
        );
        assert!(out.contains(r#"resolver = "3""#), "{out}");
    }

    #[test]
    fn lists_inherited_dependency_names() {
        let names = inherited_dependency_names(MEMBER).expect("collects");
        assert_eq!(
            names,
            ["chrono", "git2", "github-avatar", "indicatif", "tempfile"]
        );
    }

    #[test]
    fn rejects_unknown_workspace_dependency() {
        let member = "[package]\nname = \"x\"\n\n[dependencies]\nnope.workspace = true\n";
        let root: DocumentMut = ROOT.parse().expect("root fixture parses");
        let workspace = WorkspaceTables {
            package: root["workspace"]["package"]
                .as_table()
                .expect("package table"),
            dependencies: root["workspace"]["dependencies"]
                .as_table()
                .expect("dependencies table"),
        };
        let err = standalone(member, &workspace, &|_| String::new()).expect_err("must fail");
        assert!(err.to_string().contains("nope"), "{err}");
    }
}
