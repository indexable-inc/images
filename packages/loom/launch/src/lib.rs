//! Shared pieces of the loom launchers: the API-key search order, required
//! env lookup, an exec that only returns on failure, and the fork-identity
//! watcher both interactive entrypoints run under.

use std::collections::BTreeSet;
use std::net::IpAddr;
use std::os::unix::process::CommandExt;
use std::process::{Child, Command, ExitCode, ExitStatus};
use std::time::Duration;

use anyhow::Context as _;
use nix::sys::signal::{Signal, kill};
use nix::unistd::Pid;

/// Disk first, tmpfs second. `/run/secrets` is tmpfs and does not survive a
/// stop/start of a restored fork (measured live), so provisioning persists
/// the key to `/var/lib/loom` and a woken fork reads that copy.
const KEY_PATHS: [&str; 2] = [
    "/var/lib/loom/anthropic_api_key",
    "/run/secrets/anthropic_api_key",
];

/// The first non-empty key file, trailing newline stripped. Both files
/// missing or empty is an error rather than an empty key: claude would only
/// fail later with a less specific message.
pub fn anthropic_api_key() -> anyhow::Result<String> {
    let found = KEY_PATHS
        .iter()
        .filter_map(|path| std::fs::read_to_string(path).ok())
        .find(|raw| !raw.trim().is_empty());
    match found {
        Some(raw) => Ok(raw.trim_end_matches('\n').to_owned()),
        None => anyhow::bail!("no anthropic api key at {}", KEY_PATHS.join(" or ")),
    }
}

/// An env var the wrapper is required to set; absence is a packaging bug.
pub fn required_env(name: &str) -> anyhow::Result<String> {
    std::env::var(name).with_context(|| format!("{name} is not set; the nix wrapper sets it"))
}

/// Exec `cmd`, so a success never returns; the returned error is the exec
/// failure with the program named.
pub fn exec(mut cmd: Command) -> anyhow::Error {
    let program = cmd.get_program().to_owned();
    let err = cmd.exec();
    anyhow::Error::new(err).context(format!("exec {}", program.to_string_lossy()))
}

/// The set `ip -o addr show scope global` would print: every configured
/// address except loopback and link-local. A snapshot fork gets a different
/// set by construction (the guest's private IP moves with the VM), so this
/// is the identity that decides whether a resumed process is the clone.
pub fn global_addrs() -> anyhow::Result<BTreeSet<IpAddr>> {
    let addrs = nix::ifaddrs::getifaddrs().context("getifaddrs")?;
    let set = addrs
        .filter_map(|ifaddr| ifaddr.address.as_ref().and_then(ip_of))
        .filter(global_scope)
        .collect();
    Ok(set)
}

/// Reap `child`, or terminate it the moment the VM's addresses stop matching
/// `baseline` - a snapshot restore resumed this process inside a fork, and
/// the cloned session must not keep running there. `on_fork` runs once,
/// before the SIGTERM, for extra teardown the plain signal cannot express
/// (e.g. killing a zellij session server-side). A probe failure also
/// terminates: an unreadable address table cannot vouch for the identity
/// that decides whether this session may run.
pub fn watch_identity(
    mut child: Child,
    baseline: &BTreeSet<IpAddr>,
    on_fork: impl Fn(),
) -> anyhow::Result<ExitCode> {
    loop {
        if let Some(status) = child.try_wait().context("wait for child")? {
            return Ok(exit_code(status));
        }
        std::thread::sleep(Duration::from_secs(1));
        let matches_baseline = global_addrs().is_ok_and(|current| current == *baseline);
        if !matches_baseline {
            on_fork();
            terminate(&child);
            let status = child.wait().context("wait for terminated child")?;
            return Ok(exit_code(status));
        }
    }
}

fn terminate(child: &Child) {
    if let Ok(pid) = i32::try_from(child.id()) {
        // ESRCH here means the child exited between try_wait and now; wait()
        // right after still reaps it.
        let _ = kill(Pid::from_raw(pid), Signal::SIGTERM);
    }
}

fn ip_of(storage: &nix::sys::socket::SockaddrStorage) -> Option<IpAddr> {
    if let Some(v4) = storage.as_sockaddr_in() {
        Some(IpAddr::V4(v4.ip()))
    } else {
        storage.as_sockaddr_in6().map(|v6| IpAddr::V6(v6.ip()))
    }
}

fn global_scope(addr: &IpAddr) -> bool {
    match addr {
        IpAddr::V4(v4) => !v4.is_loopback() && !v4.is_link_local(),
        IpAddr::V6(v6) => !v6.is_loopback() && !v6.is_unicast_link_local(),
    }
}

pub fn exit_code(status: ExitStatus) -> ExitCode {
    if status.success() {
        ExitCode::SUCCESS
    } else {
        let code = status
            .code()
            .and_then(|c| u8::try_from(c).ok())
            .unwrap_or(1);
        ExitCode::from(code)
    }
}
