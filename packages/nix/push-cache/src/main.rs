//! `push-cache <installable|/nix/store/path>...`: archive store closures into
//! a durable local file:// binary cache directory (`$IX_PUSH_CACHE_DIR`,
//! default `~/.cache/ix-push-cache`). See default.nix for why a local cache
//! exists at all (no aarch64-linux publisher for cache.ix.dev) and the
//! unsigned-consumer caveats.
//!
//! Two modes, split by argument shape:
//!  - a /nix/store/... argument (or --paths-from FILE) is archived as-is:
//!    `nix copy` closes over runtime references itself, with no evaluation
//!    and no realisation. This is the only mode that works for paths built
//!    REMOTELY and pulled back (e.g. aarch64-linux outputs from the builder
//!    VM via `nix copy --from ssh-ng://root@vm`): the host store records no
//!    deriver for them, so a drv-closure walk finds nothing, and building
//!    the flake installable host-side dies on the platform mismatch.
//!  - a flake installable is built locally and archived with its full BUILD
//!    closure (all build-time deps' outputs, not just the runtime closure),
//!    which is what keeps kernel/mesa/toolchain intermediates warm across a
//!    closure shift.

use std::env;
use std::fs;
use std::io::Write;
use std::path::PathBuf;
use std::process::{Command, Stdio};

use anyhow::{Context, Result, bail, ensure};
use clap::Parser;

/// Archive store closures into a local file:// binary cache.
#[derive(Parser)]
struct Args {
    /// File of already-valid store paths, one per line, archived as-is.
    #[arg(long, value_name = "FILE")]
    paths_from: Option<PathBuf>,
    installables: Vec<String>,
}

/// Store paths archived as-is vs flake installables built first.
struct Split {
    direct: Vec<String>,
    flakes: Vec<String>,
}

fn split_installables(installables: Vec<String>, listed: Vec<String>) -> Split {
    let (direct, flakes): (Vec<String>, Vec<String>) = installables
        .into_iter()
        .partition(|installable| installable.starts_with("/nix/store/"));
    Split {
        direct: direct.into_iter().chain(listed).collect(),
        flakes,
    }
}

fn cache_dir(
    push_cache_dir: Option<String>,
    xdg_cache_home: Option<String>,
    home: &str,
) -> PathBuf {
    push_cache_dir.map_or_else(
        || {
            xdg_cache_home
                .map_or_else(|| PathBuf::from(home).join(".cache"), PathBuf::from)
                .join("ix-push-cache")
        },
        PathBuf::from,
    )
}

/// One renderer for the cache substituter URL. zstd over the xz default:
/// this cache lives on local disk where write time, not size, is the
/// constraint, and multi-GiB image closures under xz would dominate the
/// whole run.
fn cache_url(dir: &str) -> String {
    format!("file://{dir}?compression=zstd")
}

/// `nix copy --stdin` instead of argv: an image build closure is thousands
/// of paths, past the execve argument limit. nix skips paths whose narinfo
/// is already in the cache, so re-runs are incremental.
fn nix_copy(paths: &[String], url: &str) -> Result<()> {
    let mut child = Command::new("nix")
        .args(["copy", "--to", url, "--stdin"])
        .stdin(Stdio::piped())
        .spawn()
        .context("spawn nix copy")?;
    {
        let mut stdin = child.stdin.take().context("open nix copy stdin")?;
        for path in paths {
            writeln!(stdin, "{path}").context("write path to nix copy")?;
        }
    }
    let status = child.wait().context("wait for nix copy")?;
    ensure!(status.success(), "nix copy exited with {status}");
    Ok(())
}

fn capture_lines(command: &mut Command) -> Result<Vec<String>> {
    let program = command.get_program().to_string_lossy().into_owned();
    let output = command.output().with_context(|| format!("run {program}"))?;
    ensure!(
        output.status.success(),
        "{program} exited with {}",
        output.status
    );
    Ok(String::from_utf8(output.stdout)
        .with_context(|| format!("{program} emitted non-UTF-8 output"))?
        .lines()
        .map(str::to_owned)
        .collect())
}

/// The BUILD closure of a flake installable: requisites of the derivation
/// plus every already-realised output (--include-outputs lists only outputs
/// that exist, so nothing here forces extra builds). The .drv files
/// themselves are dropped: substitution serves outputs, and drvs
/// re-instantiate for free from the flake.
fn build_closure(installable: &str) -> Result<Vec<String>> {
    let drvs = capture_lines(Command::new("nix").args(["path-info", "--derivation", installable]))?;
    let mut seen = std::collections::HashSet::new();
    let mut paths = Vec::new();
    for drv in drvs {
        let requisites = capture_lines(Command::new("nix-store").args([
            "--query",
            "--requisites",
            "--include-outputs",
            &drv,
        ]))?;
        for path in requisites {
            let is_drv = std::path::Path::new(&path)
                .extension()
                .is_some_and(|ext| ext.eq_ignore_ascii_case("drv"));
            if !is_drv && seen.insert(path.clone()) {
                paths.push(path);
            }
        }
    }
    Ok(paths)
}

fn main() -> Result<()> {
    let args = Args::parse();

    let listed = match &args.paths_from {
        None => Vec::new(),
        Some(file) => fs::read_to_string(file)
            .with_context(|| format!("read {}", file.display()))?
            .lines()
            .map(str::trim)
            .filter(|line| !line.is_empty())
            .map(str::to_owned)
            .collect(),
    };
    let split = split_installables(args.installables, listed);
    if split.direct.is_empty() && split.flakes.is_empty() {
        bail!(
            "usage: push-cache <installable|/nix/store/path>... [--paths-from FILE]  e.g. push-cache .#packages.aarch64-linux.panes-guest-image"
        );
    }

    let home = env::var("HOME").context("HOME is not set")?;
    let dir = cache_dir(
        env::var("IX_PUSH_CACHE_DIR").ok(),
        env::var("XDG_CACHE_HOME").ok(),
        &home,
    );
    fs::create_dir_all(&dir).with_context(|| format!("create {}", dir.display()))?;
    let dir = dir.to_str().context("cache dir is not valid UTF-8")?;
    let url = cache_url(dir);

    if !split.direct.is_empty() {
        println!(
            "push-cache: copying {} store paths + runtime closure to {dir}",
            split.direct.len()
        );
        nix_copy(&split.direct, &url)?;
    }

    for installable in &split.flakes {
        println!("push-cache: building {installable}");
        let status = Command::new("nix")
            .args(["build", "--no-link", installable])
            .status()
            .context("run nix build")?;
        ensure!(status.success(), "nix build {installable} exited with {status}");

        let paths = build_closure(installable)?;
        println!("push-cache: copying {} store paths to {dir}", paths.len());
        nix_copy(&paths, &url)?;
    }

    println!(
        "push-cache: done; substituter file://{dir} is unsigned, so consumers need require-sigs = false or a separate signature"
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn split_routes_store_paths_direct_and_appends_listed() {
        let split = split_installables(
            vec![
                "/nix/store/aaa-x".to_owned(),
                ".#packages.aarch64-linux.panes-guest-image".to_owned(),
            ],
            vec!["/nix/store/bbb-y".to_owned()],
        );
        assert_eq!(
            split.direct,
            vec!["/nix/store/aaa-x".to_owned(), "/nix/store/bbb-y".to_owned()]
        );
        assert_eq!(
            split.flakes,
            vec![".#packages.aarch64-linux.panes-guest-image".to_owned()]
        );
    }

    #[test]
    fn cache_dir_prefers_explicit_then_xdg_then_home() {
        assert_eq!(
            cache_dir(Some("/tmp/pc".to_owned()), Some("/xdg".to_owned()), "/home/me"),
            PathBuf::from("/tmp/pc")
        );
        assert_eq!(
            cache_dir(None, Some("/xdg".to_owned()), "/home/me"),
            PathBuf::from("/xdg/ix-push-cache")
        );
        assert_eq!(
            cache_dir(None, None, "/home/me"),
            PathBuf::from("/home/me/.cache/ix-push-cache")
        );
    }

    #[test]
    fn cache_url_requests_zstd() {
        assert_eq!(
            cache_url("/home/me/.cache/ix-push-cache"),
            "file:///home/me/.cache/ix-push-cache?compression=zstd"
        );
    }
}
