//! Health check: the sandboxed agent's uid reaches the proxy and nothing
//! else.
//!
//! Proves the containment behaves rather than that the units exist: the
//! nftables policy table is loaded, a TCP connect to the proxy port
//! succeeds from the agent uid, and a direct attempt at the upstream's
//! port 443 from the same uid fails fast (reject, not a timeout hang).
//! Secret-independent, so it passes on a fresh VM and in CI where no key
//! is attached.
//!
//! Runs as root (the health-check runner) and re-executes itself under the
//! agent uid via systemd-run for the two probes: `--connect HOST:PORT`
//! mode attempts one TCP connect and exits by its outcome, so no netcat or
//! curl needs to enter the sandbox's PATH.
//!
//! argv: `--nft PATH --systemd-run PATH --user NAME --table NAME
//! --proxy-port N --upstream HOST`, wired by modules/services/sandboxed-agent.

use std::error::Error;
use std::net::{TcpStream, ToSocketAddrs};
use std::process::{Command, Stdio};
use std::time::Duration;
use std::{env, process};

struct Config {
    nft: String,
    systemd_run: String,
    user: String,
    table: String,
    proxy_port: u16,
    upstream: String,
}

fn main() -> Result<(), Box<dyn Error>> {
    let args: Vec<String> = env::args().skip(1).collect();
    if let [flag, target] = args.as_slice()
        && flag == "--connect"
    {
        probe(target);
    }
    let config = parse_args(&args)?;

    let table_loaded = Command::new(&config.nft)
        .args(["list", "table", "inet", &config.table])
        .stdout(Stdio::null())
        .status()?
        .success();
    if !table_loaded {
        return Err(format!("nftables table inet {} is not loaded", config.table).into());
    }

    match connect_as_agent(&config, &format!("127.0.0.1:{}", config.proxy_port))? {
        Some(0) => {}
        Some(UNREACHED) => {
            return Err(format!("the {} uid cannot reach the loopback proxy", config.user).into());
        }
        code => return Err(format!("proxy probe did not run (exit {code:?})").into()),
    }

    match connect_as_agent(&config, &format!("{}:443", config.upstream))? {
        Some(UNREACHED) => {}
        Some(0) => {
            return Err(format!(
                "the {} uid unexpectedly reached {} directly",
                config.user, config.upstream,
            )
            .into());
        }
        code => return Err(format!("upstream probe did not run (exit {code:?})").into()),
    }

    Ok(())
}

/// `--connect` mode exit code for "the probe ran and the target was
/// unreachable" -- distinct from 0 (reached) and from generic failures
/// (1, or systemd-run's own 2xx service codes), so the parent can tell
/// "confinement held" apart from "the probe never ran". Without the
/// distinction, a broken systemd-run would pass the upstream assertion.
const UNREACHED: i32 = 7;

/// `--connect` mode, running under the agent uid. A blocked resolver
/// counts as unreachable, which is the point -- DNS from the agent uid is
/// rejected like everything else.
fn probe(target: &str) -> ! {
    let reached = target
        .to_socket_addrs()
        .ok()
        .and_then(|mut addrs| addrs.next())
        .is_some_and(|addr| {
            TcpStream::connect_timeout(&addr, Duration::from_secs(5)).is_ok()
        });
    process::exit(if reached { 0 } else { UNREACHED });
}

fn connect_as_agent(config: &Config, target: &str) -> Result<Option<i32>, Box<dyn Error>> {
    let this_binary = env::current_exe()?;
    let status = Command::new(&config.systemd_run)
        .args([
            "--quiet",
            "--collect",
            "--pipe",
            "--wait",
            &format!("--uid={}", config.user),
            &format!("--gid={}", config.user),
        ])
        .arg(this_binary)
        .args(["--connect", target])
        .status()?;
    Ok(status.code())
}

fn parse_args(args: &[String]) -> Result<Config, Box<dyn Error>> {
    let mut nft = None;
    let mut systemd_run = None;
    let mut user = None;
    let mut table = None;
    let mut proxy_port = None;
    let mut upstream = None;

    let mut pairs = args.chunks_exact(2);
    if !pairs.remainder().is_empty() {
        return Err("arguments must be --flag value pairs".into());
    }
    for pair in pairs.by_ref() {
        let flag = pair[0].as_str();
        let value = pair[1].as_str();
        match flag {
            "--nft" => nft = Some(value.to_owned()),
            "--systemd-run" => systemd_run = Some(value.to_owned()),
            "--user" => user = Some(value.to_owned()),
            "--table" => table = Some(value.to_owned()),
            "--proxy-port" => proxy_port = Some(value.parse::<u16>()?),
            "--upstream" => upstream = Some(value.to_owned()),
            _ => return Err(format!("unknown argument: {flag}").into()),
        }
    }

    Ok(Config {
        nft: nft.ok_or("--nft is required")?,
        systemd_run: systemd_run.ok_or("--systemd-run is required")?,
        user: user.ok_or("--user is required")?,
        table: table.ok_or("--table is required")?,
        proxy_port: proxy_port.ok_or("--proxy-port is required")?,
        upstream: upstream.ok_or("--upstream is required")?,
    })
}
