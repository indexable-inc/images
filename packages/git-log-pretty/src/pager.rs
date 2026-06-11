//! Route long output through a pager, the way `git log` does.
//!
//! When stdout is a terminal we spawn a pager and write the rendered output to
//! its stdin, leaving our own stdout pointed at the terminal so theme detection
//! still sees a TTY. Off a TTY (a pipe, a capture, a test) we write straight to
//! stdout, so machine consumers get the exact bytes with no pager in the way.
//!
//! The pager is `$PAGER` when set (run through `sh -c`, so `PAGER="less -R"`
//! and pipelines work), falling back to `less`. An empty `$PAGER` disables
//! paging, matching git. We default `$LESS` to `FrX` when unset: quit if the
//! output fits one screen, pass raw bytes through so Nerd Font icons render
//! (and color with them), and leave the text in scrollback instead of an
//! alternate screen.

use std::io::{self, IsTerminal, Write};
use std::process::{Child, Command, Stdio};

use color_eyre::eyre::{Result, WrapErr};

/// Run `render` with a writer that is the pager's stdin on a TTY, or stdout
/// otherwise.
///
/// `paged` is `true` to allow paging (the default for log views); pass `false`
/// for `--no-pager`. A pager that the reader quits early closes the pipe, which
/// surfaces as [`io::ErrorKind::BrokenPipe`]; that is a normal "I've seen
/// enough" exit, so it is swallowed rather than reported as a failure.
pub fn paged<F>(allow: bool, render: F) -> Result<()>
where
    F: FnOnce(&mut dyn Write) -> Result<()>,
{
    if allow
        && io::stdout().is_terminal()
        && let Some(mut child) = spawn()
    {
        let result = {
            let mut stdin = child.stdin.take().expect("pager spawned with piped stdin");
            render(&mut stdin)
            // `stdin` drops here, closing the pipe so the pager sees EOF.
        };
        let result = swallow_broken_pipe(result);
        child.wait().wrap_err("failed to wait for the pager")?;
        return result;
    }

    let mut stdout = io::stdout().lock();
    swallow_broken_pipe(render(&mut stdout))
}

/// Map a broken-pipe error to success; pass every other result through.
fn swallow_broken_pipe(result: Result<()>) -> Result<()> {
    match result {
        Err(report)
            if report
                .downcast_ref::<io::Error>()
                .is_some_and(|err| err.kind() == io::ErrorKind::BrokenPipe) =>
        {
            Ok(())
        }
        other => other,
    }
}

/// Spawn the configured pager with piped stdin, or `None` when paging is
/// disabled (empty `$PAGER`) or the pager cannot be launched.
fn spawn() -> Option<Child> {
    let command = match std::env::var("PAGER") {
        Ok(value) if value.trim().is_empty() => return None,
        Ok(value) => value,
        Err(_) => "less".to_string(),
    };

    // When paging with less, force raw mode plus scrollback-friendly defaults:
    // -F quit if it fits one screen, -r pass raw bytes through, -X stay in
    // scrollback. The file-icon tree uses Nerd Font glyphs in the Unicode
    // Private Use Area, which less renders as literal `<U+E5FF>` escapes under
    // -R; -r passes them to the terminal font instead. Appending after any user
    // flags and `$LESS` means our -r wins over an inherited -R, and our output
    // is only SGR color plus printable glyphs (no cursor motion), so raw is
    // safe. Non-less pagers are left exactly as the user configured them.
    let command = if is_less(&command) {
        format!("{command} -F -r -X")
    } else {
        command
    };

    let child = Command::new("sh")
        .arg("-c")
        .arg(&command)
        .stdin(Stdio::piped())
        .spawn();

    // A missing or unlaunchable pager should not fail the command; fall back to
    // writing directly to stdout instead.
    child.ok()
}

/// Whether `command` invokes `less`, by the program's basename, so an inherited
/// `PAGER=/usr/bin/less` or `PAGER="less -S"` is still recognized.
fn is_less(command: &str) -> bool {
    command
        .split_whitespace()
        .next()
        .map(|prog| prog.rsplit('/').next().unwrap_or(prog))
        == Some("less")
}

#[cfg(test)]
mod tests {
    use color_eyre::eyre::eyre;

    use super::*;

    #[test]
    fn broken_pipe_becomes_ok() {
        let err = io::Error::new(io::ErrorKind::BrokenPipe, "reader quit");
        assert!(swallow_broken_pipe(Err(err.into())).is_ok());
    }

    #[test]
    fn other_io_error_passes_through() {
        let err = io::Error::new(io::ErrorKind::PermissionDenied, "nope");
        assert!(swallow_broken_pipe(Err(err.into())).is_err());
    }

    #[test]
    fn non_io_error_passes_through() {
        assert!(swallow_broken_pipe(Err(eyre!("unrelated"))).is_err());
    }

    #[test]
    fn recognizes_less_invocations() {
        assert!(is_less("less"));
        assert!(is_less("less -S"));
        assert!(is_less("/usr/bin/less"));
        assert!(is_less("/opt/homebrew/bin/less -R"));
    }

    #[test]
    fn other_pagers_are_left_alone() {
        assert!(!is_less("bat"));
        assert!(!is_less("moar"));
        assert!(!is_less("/usr/bin/cat"));
    }
}
