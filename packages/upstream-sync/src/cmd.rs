//! Thin subprocess helpers over external tools (`git`, `gh`, `nix`).
//!
//! Every invocation goes through these two entry points so argv is always a
//! structured list, never a hand-joined string.

use std::path::Path;
use std::process::{Command, Output};

use color_eyre::eyre::{Result, WrapErr, eyre};

/// Captured result of a completed subprocess.
#[derive(Debug)]
pub struct Captured {
    pub status: i32,
    pub stdout: String,
    pub stderr: String,
}

impl Captured {
    #[must_use]
    pub const fn ok(&self) -> bool {
        self.status == 0
    }
}

fn capture(output: &Output) -> Captured {
    Captured {
        status: output.status.code().unwrap_or(-1),
        stdout: String::from_utf8_lossy(&output.stdout).into_owned(),
        stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
    }
}

/// Run `program args...`, capturing output without failing on nonzero exit.
///
/// # Errors
/// Returns an error only when the process cannot be spawned.
pub fn complete<S: AsRef<str>>(program: &str, args: &[S]) -> Result<Captured> {
    let output = Command::new(program)
        .args(args.iter().map(AsRef::as_ref))
        .output()
        .wrap_err_with(|| format!("failed to spawn {program}"))?;
    Ok(capture(&output))
}

/// Run `program args...` in `dir`, capturing output without failing on
/// nonzero exit.
///
/// # Errors
/// Returns an error only when the process cannot be spawned.
pub fn complete_in<S: AsRef<str>>(dir: &Path, program: &str, args: &[S]) -> Result<Captured> {
    let output = Command::new(program)
        .args(args.iter().map(AsRef::as_ref))
        .current_dir(dir)
        .output()
        .wrap_err_with(|| format!("failed to spawn {program} in {}", dir.display()))?;
    Ok(capture(&output))
}

/// Run `program args...` and return trimmed stdout, failing on nonzero exit.
///
/// # Errors
/// Returns an error when the process cannot be spawned or exits nonzero.
pub fn run<S: AsRef<str>>(program: &str, args: &[S]) -> Result<String> {
    let out = complete(program, args)?;
    if !out.ok() {
        return Err(eyre!(
            "{program} {} failed ({}): {}",
            args.iter().map(AsRef::as_ref).collect::<Vec<_>>().join(" "),
            out.status,
            out.stderr.trim()
        ));
    }
    Ok(out.stdout.trim().to_owned())
}

/// Like [`run`] but executed inside `dir`.
///
/// # Errors
/// Returns an error when the process cannot be spawned or exits nonzero.
pub fn run_in<S: AsRef<str>>(dir: &Path, program: &str, args: &[S]) -> Result<String> {
    let out = complete_in(dir, program, args)?;
    if !out.ok() {
        return Err(eyre!(
            "{program} {} failed in {} ({}): {}",
            args.iter().map(AsRef::as_ref).collect::<Vec<_>>().join(" "),
            dir.display(),
            out.status,
            out.stderr.trim()
        ));
    }
    Ok(out.stdout.trim().to_owned())
}
