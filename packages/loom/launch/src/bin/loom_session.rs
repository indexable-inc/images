//! The `loom` entry inside the control VM: one deterministic zellij session,
//! lead claude on the left, the live fleet TUI (`ix ls --watch`) on the
//! right, so fork VMs are visible spinning up and down next to the agent
//! that spawns them.
//!
//! The session is named and its layout committed (`layout.kdl`), so every
//! start produces the same two panes; a rerun while the session lives
//! attaches instead of stacking a second copy.
//!
//! A snapshot restore resumes this process - and the cloned zellij server -
//! in the child VM. The identity watcher kills the cloned session there
//! (server first, then the client), so only the fork's headless `claude -p`
//! child runs in a fork; the lead pane's own watcher in `loom-lead` is the
//! second, independent line of defense.

use std::process::{Command, ExitCode, Stdio};

use anyhow::Context as _;

const SESSION: &str = "loom";

fn main() -> ExitCode {
    match run() {
        Ok(code) => code,
        Err(err) => {
            eprintln!("loom: {err:#}");
            ExitCode::FAILURE
        }
    }
}

fn run() -> anyhow::Result<ExitCode> {
    let zellij = loom_launch::required_env("LOOM_ZELLIJ_BIN")?;
    let layout = loom_launch::required_env("LOOM_LAYOUT_FILE")?;
    let baseline = loom_launch::global_addrs()?;
    let mut cmd = Command::new(&zellij);
    if session_exists(&zellij)? {
        cmd.args(["attach", SESSION]);
    } else {
        cmd.args(["--session", SESSION, "--new-session-with-layout"])
            .arg(layout);
        // A zellij layout starts panes without argv; forward this
        // invocation's arguments to the lead pane through the environment
        // (whitespace-split there, same contract as LOOM_CLAUDE_ARGS).
        let forwarded: Vec<String> = std::env::args().skip(1).collect();
        if !forwarded.is_empty() {
            cmd.env("LOOM_LEAD_ARGS", forwarded.join(" "));
        }
    }
    let child = cmd.spawn().context("spawn zellij")?;
    loom_launch::watch_identity(child, &baseline, || kill_session(&zellij))
}

/// Whether a session named `loom` exists (live or resurrectable). A
/// zellij with no sessions at all exits non-zero, which means "no".
fn session_exists(zellij: &str) -> anyhow::Result<bool> {
    let output = Command::new(zellij)
        .args(["list-sessions", "--short"])
        .stdin(Stdio::null())
        .output()
        .context("run zellij list-sessions")?;
    if !output.status.success() {
        return Ok(false);
    }
    let names = String::from_utf8_lossy(&output.stdout);
    Ok(names.lines().any(|name| name.trim() == SESSION))
}

/// Best-effort server-side teardown of the cloned session inside a fork;
/// the SIGTERM to the client right after covers a server that already died.
fn kill_session(zellij: &str) {
    let _ = Command::new(zellij)
        .args(["kill-session", SESSION])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status();
}
