//! Claude with the loom key loaded: what the remote exec path and the
//! Elixir side run inside a fork.

use std::process::{Command, ExitCode};

fn main() -> ExitCode {
    let err = match run() {
        Ok(never) => match never {},
        Err(err) => err,
    };
    eprintln!("loom-claude: {err:#}");
    ExitCode::FAILURE
}

fn run() -> anyhow::Result<std::convert::Infallible> {
    let claude = loom_launch::required_env("LOOM_CLAUDE_BIN")?;
    let key = loom_launch::anthropic_api_key()?;
    let mut cmd = Command::new(claude);
    // IS_SANDBOX here, not only in the login-shell environment: the remote
    // child runs in a non-login guest session that never sources
    // environment.variables, and claude 2.1.222 refuses
    // --dangerously-skip-permissions as root without it (measured live in
    // the template e2e: every fork child exited 1 on exactly this). The
    // fork IS the sandbox - a disposable VM - which is what the variable
    // asserts.
    cmd.args(std::env::args_os().skip(1))
        .env("ANTHROPIC_API_KEY", key)
        .env("IS_SANDBOX", "1");
    Err(loom_launch::exec(cmd))
}
