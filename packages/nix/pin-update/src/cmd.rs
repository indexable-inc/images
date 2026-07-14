//! Subprocess helpers: every external command is built from typed args and
//! checked, so a failure surfaces with the command line and its stderr.

use std::process::{Command, Stdio};

use anyhow::{Context as _, Result, bail};

/// Render a command for error messages.
fn describe(cmd: &Command) -> String {
    std::iter::once(cmd.get_program())
        .chain(cmd.get_args())
        .map(|part| part.to_string_lossy().into_owned())
        .collect::<Vec<_>>()
        .join(" ")
}

/// Run `cmd` capturing stdout; fail with stderr attached on a non-zero exit.
pub fn stdout(cmd: &mut Command) -> Result<String> {
    let what = describe(cmd);
    let output = cmd
        .stdin(Stdio::null())
        .output()
        .with_context(|| format!("spawning `{what}`"))?;
    if !output.status.success() {
        bail!(
            "`{what}` failed ({}):\n{}",
            output.status,
            String::from_utf8_lossy(&output.stderr)
        );
    }
    String::from_utf8(output.stdout).with_context(|| format!("`{what}` wrote non-UTF-8 stdout"))
}

/// Run `cmd` with inherited stdio (progress streams to the caller); fail on
/// a non-zero exit.
pub fn run(cmd: &mut Command) -> Result<()> {
    let what = describe(cmd);
    let status = cmd.status().with_context(|| format!("spawning `{what}`"))?;
    if !status.success() {
        bail!("`{what}` failed ({status})");
    }
    Ok(())
}
