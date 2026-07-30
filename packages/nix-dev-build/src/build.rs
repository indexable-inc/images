//! Driving meson and ninja: configure the build directory once, then build.

use std::fs;
use std::io;
use std::os::fd::AsFd;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use anyhow::{Context, Result, bail};
use serde::Deserialize;

/// What meson wrote about an existing build directory. Only the source path and
/// the error flag matter here: a build directory configured against a different
/// checkout, or left broken by a failed configure, produces confusing ninja
/// output rather than an obvious error, so both are checked before building.
#[derive(Deserialize)]
struct MesonInfo {
    directories: MesonDirectories,
    error: bool,
}

#[derive(Deserialize)]
struct MesonDirectories {
    source: PathBuf,
}

pub enum State {
    /// No build directory yet; meson has to configure one.
    Absent,
    /// Configured against this checkout and usable.
    Ready,
    /// Configured against a different checkout.
    ForeignSource(PathBuf),
    /// Present but not usable: no meson-info at all, or meson recorded a
    /// configure error.
    Unusable(String),
}

/// Read the state of `build_dir` without touching it.
pub fn state(build_dir: &Path, checkout: &Path) -> Result<State> {
    if !build_dir.exists() {
        return Ok(State::Absent);
    }
    let info_path = build_dir.join("meson-info/meson-info.json");
    let text = match fs::read_to_string(&info_path) {
        Ok(text) => text,
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            return Ok(State::Unusable(format!(
                "{} exists but has no {}",
                build_dir.display(),
                "meson-info/meson-info.json"
            )));
        }
        Err(error) => return Err(error).with_context(|| format!("reading {}", info_path.display())),
    };
    let info: MesonInfo =
        serde_json::from_str(&text).with_context(|| format!("parsing {}", info_path.display()))?;
    if info.error {
        return Ok(State::Unusable(format!(
            "meson recorded a configure error in {}",
            build_dir.display()
        )));
    }
    // Canonicalize both sides: meson stores the path it was given, which may
    // reach the same tree through a different symlink than the caller used.
    let recorded = info
        .directories
        .source
        .canonicalize()
        .unwrap_or(info.directories.source);
    if recorded == checkout {
        Ok(State::Ready)
    } else {
        Ok(State::ForeignSource(recorded))
    }
}

/// `meson setup`, from inside the checkout so meson infers the source directory.
/// `wipe` reuses the options recorded by the previous configure, which is why it
/// is only valid on a directory that already exists.
pub fn setup(checkout: &Path, build_dir: &Path, wipe: bool, quiet: bool) -> Result<Duration> {
    let mut command = Command::new("meson");
    command.current_dir(checkout).arg("setup");
    if wipe {
        command.arg("--wipe");
    }
    command.arg(build_dir);
    run(command, "meson setup", quiet)
}

/// `ninja -C <build_dir> <target>`. Ninja's own output goes straight through so
/// the caller watches it compile.
pub fn ninja(build_dir: &Path, target: &str, extra: &[String], quiet: bool) -> Result<Duration> {
    let mut command = Command::new("ninja");
    command.arg("-C").arg(build_dir).arg(target).args(extra);
    run(command, "ninja", quiet)
}

/// Run to completion, inheriting stdio, and turn a non-zero exit into an error.
/// There is deliberately no fallback: a failed ninja build means the tree does
/// not compile, and quietly reaching for `nix build` would hide that.
fn run(mut command: Command, what: &str, quiet: bool) -> Result<Duration> {
    command.stdout(progress_sink(quiet)?).stderr(Stdio::inherit());
    let started = Instant::now();
    let status = command.status().with_context(|| {
        format!("spawning {what} (is this running inside the checkout's dev shell?)")
    })?;
    let elapsed = started.elapsed();
    match status.code() {
        Some(0) => Ok(elapsed),
        Some(code) => bail!("{what} exited {code}"),
        None => bail!("{what} was terminated by a signal"),
    }
}

/// Where a child's progress output goes. Under `--json` stdout carries one
/// document and nothing else, so progress is redirected to stderr rather than
/// interleaved into the document.
fn progress_sink(quiet: bool) -> Result<Stdio> {
    if !quiet {
        return Ok(Stdio::inherit());
    }
    let stderr = io::stderr()
        .as_fd()
        .try_clone_to_owned()
        .context("duplicating stderr for build progress")?;
    Ok(Stdio::from(stderr))
}
