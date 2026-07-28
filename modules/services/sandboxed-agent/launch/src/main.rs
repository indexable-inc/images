//! Root's entry point to a sandboxed agent, installed on PATH under the
//! agent's command name (the real binary stays off PATH).
//!
//! Drops from the operator's root shell into the unprivileged agent user
//! via systemd-run (a fresh cgroup-scoped pty session in the caller's
//! working directory) and execs the confined program with the environment
//! the module configured -- typically a base URL pointing at the loopback
//! key-injecting proxy and a decoy credential. `NoNewPrivileges` pins the
//! uid for every descendant, which is exactly the identity the
//! sandboxed-agent nftables egress policy keys on.
//!
//! Configuration arrives through the environment, baked in by the module's
//! `makeBinaryWrapper` call (the config-launch idiom):
//! `SANDBOXED_AGENT_SYSTEMD_RUN`, `SANDBOXED_AGENT_USER`,
//! `SANDBOXED_AGENT_PROGRAM`, and one `SANDBOXED_AGENT_SETENV_<NAME>` per
//! session variable. Command-line arguments pass through to the confined
//! program untouched.

use std::os::unix::fs::MetadataExt;
use std::os::unix::process::CommandExt;
use std::process::Command;
use std::{env, fs, process};

const SETENV_PREFIX: &str = "SANDBOXED_AGENT_SETENV_";

fn required(name: &str) -> String {
    env::var(name).unwrap_or_else(|_| {
        eprintln!("sandboxed-agent-launch: {name} is not set; this binary only works behind the module's wrapper");
        process::exit(1);
    })
}

fn main() {
    let systemd_run = required("SANDBOXED_AGENT_SYSTEMD_RUN");
    let user = required("SANDBOXED_AGENT_USER");
    let program = required("SANDBOXED_AGENT_PROGRAM");

    // Effective uid without unsafe: /proc/self is owned by the process's
    // euid, and this launcher only ever runs on the Linux VM the module
    // configures.
    let euid = fs::metadata("/proc/self").map(|meta| meta.uid());
    if !matches!(euid, Ok(0)) {
        eprintln!(
            "run this from the VM's root shell (ix shell); it drops to the sandboxed '{user}' user itself"
        );
        process::exit(1);
    }

    let mut command = Command::new(systemd_run);
    command.args([
        "--quiet",
        "--collect",
        "--wait",
        "--pty",
        "--same-dir",
        &format!("--uid={user}"),
        &format!("--gid={user}"),
        "--property=NoNewPrivileges=yes",
    ]);
    // The transient unit starts from an empty environment, so the caller's
    // terminal type must ride along explicitly for the agent's TUI.
    let term = env::var("TERM").unwrap_or_else(|_| "xterm-256color".to_owned());
    command.arg(format!("--setenv=TERM={term}"));
    for (name, value) in env::vars() {
        if let Some(session_name) = name.strip_prefix(SETENV_PREFIX) {
            command.arg(format!("--setenv={session_name}={value}"));
        }
    }
    command.arg(program);
    command.args(env::args().skip(1));

    // Replacing this process is the point: the wrapper vanishes and
    // systemd-run owns the pty. exec only returns on failure.
    let err = command.exec();
    eprintln!("sandboxed-agent-launch: exec systemd-run failed: {err}");
    process::exit(1);
}
