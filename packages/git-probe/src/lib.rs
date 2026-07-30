//! A one-shot, best-effort `git` invocation.
//!
//! Several tools want to report git state (a revision, a dirty flag)
//! without treating "not a git checkout" as an error: an unpacked tarball or
//! a bare directory should still work, just without that state. [`output`]
//! is the shared shape for that: run one `git` subcommand in a directory and
//! return its trimmed stdout, or `None` if `git` could not be spawned, the
//! directory is not a repository, or the subcommand exited non-zero.

#![forbid(unsafe_code)]

use std::path::Path;
use std::process::Command;

/// Run `git <args>` in `dir`, returning trimmed stdout on success.
///
/// Returns `None` on any failure: `git` missing from `PATH`, `dir` not a git
/// working tree, or a non-zero exit. Callers that need to distinguish those
/// cases should shell out themselves; this is for the common case of a
/// best-effort status line.
#[must_use]
pub fn output(dir: &Path, args: &[&str]) -> Option<String> {
    let output = Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(args)
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    Some(String::from_utf8_lossy(&output.stdout).trim().to_owned())
}
