//! Re-entering this process inside the checkout's own dev shell.
//!
//! The compiler, meson, ninja and every library nix links against come from
//! `devShells` in the checkout's flake, which is upstream's own development
//! environment. Re-execing through it means this tool never has to know a
//! dependency list or set `PKG_CONFIG_PATH`, and a caller who already entered
//! the shell by hand pays nothing.

use std::convert::Infallible;
use std::env;
use std::ffi::OsString;
use std::os::unix::process::CommandExt;
use std::path::Path;
use std::process::Command;

use anyhow::{Context, Result};

use crate::identity;

/// Set on the re-exec so the inner run builds instead of entering a second
/// shell. Also the escape hatch: setting it by hand makes this tool run meson
/// and ninja from the ambient environment.
const IN_SHELL: &str = "NIX_DEV_BUILD_IN_SHELL";

pub fn inside() -> bool {
    env::var_os(IN_SHELL).is_some()
}

/// Replace this process with itself running under `nix develop`. Replacing
/// rather than spawning keeps one process in the pipeline, so an interrupt
/// reaches ninja directly.
pub fn reenter(checkout: &Path, shell: &str, args: &[OsString]) -> Result<Infallible> {
    let flake = format!("{}#{shell}", checkout.display());
    let self_exe = env::current_exe().context("locating this executable to re-exec it")?;

    eprintln!("nix-dev-build: entering the dev shell (nix develop {flake})");

    let mut command = Command::new("nix");
    command
        .arg("develop")
        // Passed explicitly so this works against a nix whose config does not
        // already enable them; `nix develop` is otherwise refused outright.
        .args(["--extra-experimental-features", "nix-command flakes"])
        .arg(&flake)
        .arg("--command")
        .arg(&self_exe)
        .args(args)
        .env(IN_SHELL, "1");
    if let Some(nix) = identity::nix_on_path() {
        command.env(identity::AMBIENT_NIX, nix);
    }

    let error = command.exec();
    Err(error).with_context(|| format!("running nix develop {flake}"))
}
