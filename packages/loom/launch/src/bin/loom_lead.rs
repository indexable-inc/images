//! The lead claude session: what the left zellij pane runs. Load the key,
//! append the loom system prompt, then run claude under the identity
//! watcher.
//!
//! A snapshot restore resumes this process in the child VM. The watcher
//! compares the VM's global addresses to a baseline once a second and
//! terminates claude when they differ, so the cloned lead session exits in
//! the fork; the original session keeps running because its addresses never
//! change.
//!
//! `LOOM_LEAD_ARGS` carries extra claude argv (whitespace-split) from the
//! `loom` entrypoint: a zellij layout starts panes without argv, so the
//! session bootstrap forwards its own arguments through the environment.

use std::process::{Command, ExitCode};

use anyhow::Context as _;

fn main() -> ExitCode {
    match run() {
        Ok(code) => code,
        Err(err) => {
            eprintln!("loom-lead: {err:#}");
            ExitCode::FAILURE
        }
    }
}

fn run() -> anyhow::Result<ExitCode> {
    let claude = loom_launch::required_env("LOOM_CLAUDE_BIN")?;
    let prompt = loom_launch::required_env("LOOM_PROMPT_FILE")?;
    let key = loom_launch::anthropic_api_key()?;
    let forwarded = std::env::var("LOOM_LEAD_ARGS").unwrap_or_default();
    let baseline = loom_launch::global_addrs()?;
    let child = Command::new(claude)
        .arg(format!("--append-system-prompt-file={prompt}"))
        .args(forwarded.split_whitespace())
        .args(std::env::args_os().skip(1))
        .env("ANTHROPIC_API_KEY", key)
        .env("IS_SANDBOX", "1")
        .spawn()
        .context("spawn claude")?;
    loom_launch::watch_identity(child, &baseline, || {})
}
