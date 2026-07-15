//! Captured-output subprocess runner: every nix/ssh interaction goes through
//! one function so a failure always names the command line and its stderr.

use std::process::Command;

use anyhow::{Context, Result, bail};

use crate::plan::Invocation;

pub struct Completed {
    pub code: i32,
    pub stdout: String,
    pub stderr: String,
}

/// Run the invocation to completion, capturing output. Errors only on spawn
/// failure or death by signal; a non-zero exit is a normal `Completed`.
pub fn run(invocation: &Invocation) -> Result<Completed> {
    let mut command = Command::new(invocation.program);
    command.args(&invocation.args);
    for variable in &invocation.env {
        command.env(variable.name, &variable.value);
    }
    let output = command
        .output()
        .with_context(|| format!("spawning `{invocation}` (is it installed?)"))?;
    let Some(code) = output.status.code() else {
        bail!("`{invocation}` was terminated by a signal");
    };
    Ok(Completed {
        code,
        stdout: String::from_utf8_lossy(&output.stdout).into_owned(),
        stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
    })
}

/// Run the invocation and require exit 0; returns trimmed stdout. A non-zero
/// exit becomes an error carrying the command line and stderr.
pub fn succeed(invocation: &Invocation) -> Result<String> {
    let completed = run(invocation)?;
    if completed.code != 0 {
        bail!(
            "`{invocation}` exited {}: {}",
            completed.code,
            completed.stderr.trim()
        );
    }
    Ok(completed.stdout.trim().to_owned())
}
