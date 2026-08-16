//! Run `loom-claude` inside a fork VM at a given cwd, over `ix shell`.

use std::process::{Command, ExitCode};

use anyhow::Context as _;

fn main() -> ExitCode {
    let err = match run() {
        Ok(never) => match never {},
        Err(err) => err,
    };
    eprintln!("loom-remote-claude: {err:#}");
    ExitCode::FAILURE
}

fn run() -> anyhow::Result<std::convert::Infallible> {
    const USAGE: &str = "usage: loom-remote-claude <vm> <cwd> [claude args...]";
    let ix = loom_launch::required_env("LOOM_IX_BIN")?;
    let mut args = std::env::args_os().skip(1);
    let vm = args.next().context(USAGE)?;
    let cwd = args.next().context(USAGE)?;
    // /bin/sh, not $PATH sh: the fork is NixOS, where /bin/sh is one of the
    // two blessed impure paths, and the remote session's PATH is not ours to
    // assume. The inner script cds before exec because `ix shell` has no
    // working-directory flag.
    let mut cmd = Command::new(ix);
    cmd.arg("shell")
        .arg(&vm)
        .args(["--noninteractive", "--", "/bin/sh", "-c"])
        .arg(r#"cd "$1" && shift && exec "$@""#)
        .arg("sh")
        .arg(&cwd)
        .arg("loom-claude")
        .args(args);
    Err(loom_launch::exec(cmd))
}
