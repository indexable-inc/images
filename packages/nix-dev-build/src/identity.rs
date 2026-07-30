//! Telling the binary just built apart from the `nix` already on PATH.
//!
//! A checkout build and a released nix print the same `--version` string, so a
//! number measured with the wrong one looks right. The path is the only thing
//! that distinguishes them, so every report names it.

use std::env;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::Command;

use serde::Serialize;

/// Carries the ambient `nix` across the dev shell boundary. PATH inside the
/// shell holds the bootstrap client the flake supplies, so resolving `nix`
/// there would compare the build against that rather than against the binary
/// the operator gets by typing `nix`.
pub const AMBIENT_NIX: &str = "NIX_DEV_BUILD_AMBIENT_NIX";

/// The freshly built binary.
#[derive(Serialize)]
pub struct Built {
    pub path: PathBuf,
    pub version: Option<String>,
}

/// The `nix` a plain shell would run, and whether it is indistinguishable from
/// the build by version string alone.
#[derive(Serialize)]
pub struct Ambient {
    pub path: PathBuf,
    pub version: Option<String>,
    pub same_version_string: bool,
}

impl Built {
    /// `None` when the ninja target does not name an executable under the build
    /// directory, which is the normal case for library and test targets.
    pub fn find(build_dir: &Path, target: &str) -> Option<Self> {
        let path = build_dir.join(target);
        if !is_executable(&path) {
            return None;
        }
        let version = version_of(&path);
        Some(Self { path, version })
    }
}

impl Ambient {
    pub fn find(built: &Built) -> Option<Self> {
        let path = match env::var_os(AMBIENT_NIX) {
            Some(recorded) => PathBuf::from(recorded),
            None => nix_on_path()?,
        };
        if path == built.path {
            return None;
        }
        let version = version_of(&path);
        Some(Self {
            same_version_string: version.is_some() && version == built.version,
            path,
            version,
        })
    }
}

/// The `nix` a plain shell would run, resolved from the current PATH. Called
/// from the outer process, before the dev shell rewrites PATH.
pub fn nix_on_path() -> Option<PathBuf> {
    on_path("nix")
}

/// `<binary> --version`, trimmed to its first line. `None` when the binary will
/// not run, which is informative in itself and never worth failing over.
fn version_of(binary: &Path) -> Option<String> {
    let output = Command::new(binary).arg("--version").output().ok()?;
    if !output.status.success() {
        return None;
    }
    let text = String::from_utf8_lossy(&output.stdout);
    Some(text.lines().next()?.trim().to_owned())
}

/// First executable named `name` in PATH.
fn on_path(name: &str) -> Option<PathBuf> {
    env::split_paths(&env::var_os("PATH")?)
        .map(|directory| directory.join(name))
        .find(|candidate| is_executable(candidate))
}

fn is_executable(path: &Path) -> bool {
    path.metadata()
        .is_ok_and(|meta| meta.is_file() && meta.permissions().mode() & 0o111 != 0)
}
