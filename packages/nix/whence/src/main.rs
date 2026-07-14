//! `whence <path>`: deployed config file -> defining nix source line (#2416).
//!
//! Reads the provenance manifest that modules/home/provenance.nix and
//! modules/darwin/provenance.nix bake into each generation (deployed path ->
//! { file, line, rev, drv, source, definitions, settings }), so the answer
//! comes from the live profile with zero eval. A path no manifest knows about
//! falls back to `nix-store -q --deriver` on the resolved store path.

use std::collections::BTreeMap;
use std::env;
use std::fs;
use std::path::{Component, Path, PathBuf};
use std::process::{Command, ExitCode};

use anyhow::{Context, Result};
use clap::Parser;
use serde::Deserialize;

/// Deployed config file -> defining nix source line, from the generation's
/// provenance manifest.
#[derive(Parser)]
struct Args {
    /// Deployed path to look up.
    path: String,
}

#[derive(Deserialize, Default)]
struct Manifest {
    #[serde(default)]
    files: BTreeMap<String, Entry>,
}

#[derive(Deserialize)]
struct Entry {
    rev: Option<String>,
    file: Option<String>,
    line: Option<u64>,
    #[serde(default)]
    definitions: Vec<Site>,
    #[serde(default)]
    settings: Vec<SettingChain>,
    source: Option<String>,
    drv: Option<String>,
}

#[derive(Deserialize)]
struct Site {
    file: String,
    line: Option<u64>,
}

#[derive(Deserialize)]
struct SettingChain {
    option: String,
    #[serde(default)]
    definitions: Vec<Site>,
}

/// Definition sites are store paths of the flake copy
/// (/nix/store/<hash>-source/...); strip the copy prefix so sites print
/// repo-relative.
fn clean_site(file: &str) -> String {
    lazy_regex::regex_replace!(r"^/nix/store/[a-z0-9]{32}-[^/]+/", file, "").into_owned()
}

fn format_site(site: &Site) -> String {
    site.line.map_or_else(
        || clean_site(&site.file),
        |line| format!("{}:{line}", clean_site(&site.file)),
    )
}

/// Manifests of the live generations: the home-manager profile's (XDG
/// location, plus the pre-XDG per-user profile older installs still use)
/// and, on darwin, the running system's.
fn manifest_paths() -> Vec<PathBuf> {
    let state_home = env::var("XDG_STATE_HOME").map_or_else(
        |_| Path::new(&env::var("HOME").unwrap_or_default()).join(".local/state"),
        PathBuf::from,
    );
    let user = env::var("USER").unwrap_or_default();
    [
        state_home.join("nix/profiles/home-manager/provenance.json"),
        PathBuf::from(format!(
            "/nix/var/nix/profiles/per-user/{user}/home-manager/provenance.json"
        )),
        PathBuf::from("/run/current-system/provenance.json"),
    ]
    .into_iter()
    .filter(|path| path.exists())
    .collect()
}

fn print_entry(path: &str, entry: &Entry) {
    let rev = entry.rev.as_deref().unwrap_or("unknown rev");
    let file = entry.file.as_deref().unwrap_or("?");
    let line = entry
        .line
        .map_or_else(|| "?".to_owned(), |line| line.to_string());
    println!("{path}");
    println!("  {}:{line} @ {rev}", clean_site(file));
    if entry.definitions.len() > 1 {
        println!("  defined via:");
        for site in &entry.definitions {
            println!("    {}", format_site(site));
        }
    }
    for chain in &entry.settings {
        println!("  {}:", chain.option);
        for site in &chain.definitions {
            println!("    {}", format_site(site));
        }
    }
    if let Some(source) = &entry.source {
        println!("  source: {source}");
    }
    if let Some(drv) = &entry.drv {
        println!("  drv: {drv}");
    }
}

/// Unmanifested store path: the store's own deriver link is the only
/// provenance left.
fn fallback(resolved: &str) -> Result<ExitCode> {
    println!("no provenance manifest entry for {resolved}");
    let deriver = Command::new("nix-store")
        .args(["-q", "--deriver", resolved])
        .output()
        .context("run nix-store -q --deriver")?;
    let out = String::from_utf8_lossy(&deriver.stdout).trim().to_owned();
    if deriver.status.success() && !out.is_empty() && out != "unknown-deriver" {
        println!("  deriver: {out}");
        Ok(ExitCode::SUCCESS)
    } else {
        println!("  no deriver recorded either (not built locally, or not a store path)");
        Ok(ExitCode::FAILURE)
    }
}

/// Logical absolute path (no symlink resolution): manifest keys are
/// deployment targets, which are themselves symlinks into the store. `~` and
/// `..` resolve lexically, matching how the manifest records its keys.
fn logical_absolute(path: &str, home: &str, cwd: &Path) -> PathBuf {
    let expanded = if path == "~" {
        PathBuf::from(home)
    } else if let Some(rest) = path.strip_prefix("~/") {
        Path::new(home).join(rest)
    } else {
        PathBuf::from(path)
    };
    let joined = if expanded.is_absolute() {
        expanded
    } else {
        cwd.join(expanded)
    };
    let mut normalized = PathBuf::new();
    for component in joined.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                normalized.pop();
            }
            other => normalized.push(other),
        }
    }
    normalized
}

/// Candidate manifest keys for a path: the logical and resolved forms, each
/// with a `$HOME`-stripped variant (home-manager keys are `$HOME`-relative,
/// system keys absolute), deduplicated preserving order.
fn candidate_keys(logical: &str, resolved: &str, home: &str) -> Vec<String> {
    let mut keys = Vec::new();
    for path in [logical, resolved] {
        for key in [Some(path.to_owned()), home_relative(path, home)]
            .into_iter()
            .flatten()
        {
            if !keys.contains(&key) {
                keys.push(key);
            }
        }
    }
    keys
}

fn home_relative(path: &str, home: &str) -> Option<String> {
    path.strip_prefix(home)
        .and_then(|rest| rest.strip_prefix('/'))
        .map(str::to_owned)
}

/// A matched manifest row: the key it was found under and its entry.
struct Row<'manifest> {
    key: &'manifest str,
    entry: &'manifest Entry,
}

/// The manifest row for a path: a direct key match first, else a match by
/// store payload (`source` equal to, or a directory prefix of, the resolved
/// path).
fn lookup<'manifest>(
    manifest: &'manifest Manifest,
    keys: &[String],
    resolved: &str,
) -> Option<Row<'manifest>> {
    keys.iter()
        .find_map(|key| manifest.files.get_key_value(key))
        .or_else(|| {
            manifest.files.iter().find(|(_, entry)| {
                entry.source.as_deref().is_some_and(|source| {
                    resolved == source
                        || resolved
                            .strip_prefix(source)
                            .is_some_and(|rest| rest.starts_with('/'))
                })
            })
        })
        .map(|(key, entry)| Row { key, entry })
}

fn run() -> Result<ExitCode> {
    let args = Args::parse();
    // A trailing slash on HOME would make the `$HOME/` prefix tests miss
    // every home file.
    let home_var = env::var("HOME").context("HOME is not set")?;
    let home = home_var.trim_end_matches('/');
    let cwd = env::current_dir().context("read current directory")?;

    let logical_path = logical_absolute(&args.path, home, &cwd);
    let logical = logical_path
        .to_str()
        .context("path is not valid UTF-8")?
        .to_owned();
    // Fully resolved payload, for matching by store path and the fallback.
    let resolved = if logical_path.exists() {
        let canonical = fs::canonicalize(&logical_path)
            .with_context(|| format!("resolve {logical}"))?;
        canonical
            .to_str()
            .context("resolved path is not valid UTF-8")?
            .to_owned()
    } else {
        logical.clone()
    };

    let keys = candidate_keys(&logical, &resolved, home);
    for manifest_path in manifest_paths() {
        let raw = fs::read_to_string(&manifest_path)
            .with_context(|| format!("read {}", manifest_path.display()))?;
        let manifest: Manifest = serde_json::from_str(&raw)
            .with_context(|| format!("parse {}", manifest_path.display()))?;
        if let Some(row) = lookup(&manifest, &keys, &resolved) {
            print_entry(row.key, row.entry);
            return Ok(ExitCode::SUCCESS);
        }
    }

    fallback(&resolved)
}

// clone:ignore -- the repo-idiomatic anyhow entry point (run, report the
// error, exit nonzero); every CLI here spells it the same way.
fn main() -> ExitCode {
    match run() {
        Ok(code) => code,
        Err(err) => {
            eprintln!("whence: {err:#}");
            ExitCode::FAILURE
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clean_site_strips_store_copy_prefix() {
        assert_eq!(
            clean_site("/nix/store/abcdefghijklmnopqrstuvwxyz012345-source/lib/per-system.nix"),
            "lib/per-system.nix"
        );
        assert_eq!(clean_site("lib/per-system.nix"), "lib/per-system.nix");
    }

    #[test]
    fn format_site_appends_line_when_present() {
        let with_line = Site {
            file: "modules/a.nix".to_owned(),
            line: Some(3),
        };
        let without_line = Site {
            file: "modules/a.nix".to_owned(),
            line: None,
        };
        assert_eq!(format_site(&with_line), "modules/a.nix:3");
        assert_eq!(format_site(&without_line), "modules/a.nix");
    }

    #[test]
    fn logical_absolute_expands_tilde_and_parent_dirs() {
        let cwd = Path::new("/work");
        assert_eq!(
            logical_absolute("~/x/../y", "/home/me", cwd),
            PathBuf::from("/home/me/y")
        );
        assert_eq!(
            logical_absolute("rel/./file", "/home/me", cwd),
            PathBuf::from("/work/rel/file")
        );
    }

    #[test]
    fn candidate_keys_include_home_relative_variants() {
        let keys = candidate_keys(
            "/home/me/.zshrc",
            "/nix/store/aaa-zshrc",
            "/home/me",
        );
        assert_eq!(
            keys,
            vec![
                "/home/me/.zshrc".to_owned(),
                ".zshrc".to_owned(),
                "/nix/store/aaa-zshrc".to_owned(),
            ]
        );
    }

    #[test]
    fn lookup_prefers_direct_key_then_source_prefix() {
        let manifest: Manifest = serde_json::from_str(
            r#"{"files": {
                ".zshrc": {"file": "modules/zsh.nix", "line": 4},
                "etc/app": {"file": "modules/app.nix", "source": "/nix/store/bbb-app"}
            }}"#,
        )
        .expect("manifest fixture parses");

        let direct = lookup(&manifest, &[".zshrc".to_owned()], "/nix/store/zzz")
            .expect("direct key matches");
        assert_eq!(direct.key, ".zshrc");

        let by_source = lookup(&manifest, &[], "/nix/store/bbb-app/config.toml")
            .expect("source prefix matches");
        assert_eq!(by_source.key, "etc/app");

        assert!(lookup(&manifest, &[], "/nix/store/bbb-application").is_none());
    }
}
