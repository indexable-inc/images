//! Lend a GitHub credential to a remote host for the life of one ssh
//! session, the way `ssh -A` lends a signing key.
//!
//! Why not just `ssh -A`: a forwarded agent only signs, and GitHub refuses
//! public-key authentication from our fleet's addresses while accepting the
//! same key from a workstation. So the fleet needs a token over HTTPS, and
//! the only supported ways to get one there today either write it to disk
//! or put it in a command line on a shared box. This carries the token the
//! way the agent carries a key: over a socket, for the session, leaving
//! nothing behind.

mod helper;
mod protocol;
mod serve;
mod socket;
mod token;

use std::path::PathBuf;
use std::process::Command;

use clap::{Parser, Subcommand};
use color_eyre::eyre::{Context, Result, bail};

#[derive(Parser)]
#[command(
    name = "ix-credential",
    about = "Lend a GitHub credential over a forwarded socket, the way ssh -A lends a key"
)]
struct Cli {
    #[command(subcommand)]
    command: Action,
}

#[derive(Subcommand)]
enum Action {
    /// Answer credential requests on a socket. Runs on the workstation
    /// that holds the credential.
    Serve {
        /// Where to listen. Defaults to the derived per-uid path.
        #[arg(long)]
        socket: Option<PathBuf>,
        /// Hosts this agent will answer for. Repeatable.
        #[arg(long = "allow-host", default_values_t = [String::from("github.com")])]
        allow_host: Vec<String>,
    },
    /// git's credential helper protocol on stdin and stdout. Runs on the
    /// borrowing host; register it as `credential.helper`.
    Helper {
        /// git passes `get`, `store` or `erase`.
        operation: String,
    },
    /// Serve, forward the socket to `host`, and run there until the ssh
    /// session ends. The loan lives exactly as long as this command.
    Lend {
        /// The ssh destination to lend to.
        host: String,
        /// Command to run on the remote. An interactive shell if omitted.
        #[arg(last = true)]
        remote_command: Vec<String>,
    },
    /// Print the socket path this host derives, so a long-lived process can
    /// agree with the helper without being told.
    SocketPath,
    /// git's credential helper protocol answered from a token file the host
    /// already holds, for a VM provisioned from the ix secret store rather
    /// than borrowing from a workstation. Register as `credential.helper`.
    TokenHelper {
        /// The file holding the token, e.g. a delivered secret.
        #[arg(long)]
        token_file: PathBuf,
        /// Hosts this will answer for. Repeatable.
        #[arg(long = "allow-host", default_values_t = [String::from("github.com")])]
        allow_host: Vec<String>,
        /// The username to pair with the token. GitHub ignores it for PATs
        /// and app tokens, but it must be present.
        #[arg(long, default_value = "x-access-token")]
        username: String,
        /// git passes `get`, `store` or `erase`.
        operation: String,
    },
    /// Exit nonzero, naming the cause, if a token file cannot produce a
    /// credential. The preflight for a unit about to need one.
    TokenCheck {
        /// The file holding the token.
        #[arg(long)]
        token_file: PathBuf,
    },
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    match cli.command {
        // The helper is spawned by git for every authenticated fetch, so it
        // skips the report handler: a panic hook that pretty-prints is
        // startup cost on a hot path with nothing to pretty-print.
        Action::Helper { operation } => helper::run(&operation),
        // Spawned by git per authenticated fetch, so it skips the report
        // handler for the same reason `Helper` does.
        Action::TokenHelper {
            token_file,
            allow_host,
            username,
            operation,
        } => token::helper(&operation, &token_file, &allow_host, &username),
        Action::TokenCheck { token_file } => {
            color_eyre::install()?;
            token::check(&token_file)
        }
        Action::SocketPath => {
            println!("{}", socket::path().display());
            Ok(())
        }
        Action::Serve { socket, allow_host } => {
            color_eyre::install()?;
            let path = socket.unwrap_or_else(socket::path);
            serve::run(&path, &allow_host, &serve::GhResolver)
        }
        Action::Lend {
            host,
            remote_command,
        } => {
            color_eyre::install()?;
            lend(&host, &remote_command)
        }
    }
}

/// Lend for one session: serve locally, forward the socket, run, tear down.
fn lend(host: &str, remote_command: &[String]) -> Result<()> {
    let local = socket::path();
    let remote = remote_socket_path(host)?;

    let allow = vec![String::from("github.com")];
    let local_for_thread = local.clone();
    // The agent dies with this process, so the loan cannot outlive the
    // session even if ssh is killed rather than exiting.
    let agent =
        std::thread::spawn(move || serve::run(&local_for_thread, &allow, &serve::GhResolver));

    let status = Command::new("ssh")
        .arg("-R")
        .arg(format!("{}:{}", remote.display(), local.display()))
        .arg(host)
        .args(remote_command)
        .status()
        .wrap_err("running ssh")?;

    let _ = std::fs::remove_file(&local);
    if agent.is_finished() {
        // The agent only returns on a bind failure, which is the
        // interesting error to surface over ssh's exit code.
        agent
            .join()
            .map_err(|_| color_eyre::eyre::eyre!("credential agent panicked"))??;
    }

    if status.success() {
        Ok(())
    } else {
        bail!("ssh exited with {status}");
    }
}

/// The remote path is derived from the remote uid, so ask for it rather
/// than assuming the account. One extra round trip beats a socket bound
/// where the helper will not look for it.
fn remote_socket_path(host: &str) -> Result<PathBuf> {
    let output = Command::new("ssh")
        .args([host, "id", "-u"])
        .output()
        .wrap_err("asking the remote for its uid")?;
    if !output.status.success() {
        bail!(
            "could not read the remote uid from {host}: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }

    let uid = String::from_utf8(output.stdout)?;
    let uid = uid.trim();
    if uid.is_empty() || !uid.bytes().all(|byte| byte.is_ascii_digit()) {
        bail!("{host} answered `id -u` with {uid:?}");
    }
    Ok(PathBuf::from(format!("/run/ix-credential/{uid}.sock")))
}

#[cfg(test)]
mod tests {
    use clap::CommandFactory;

    use super::*;

    #[test]
    fn the_cli_is_well_formed() {
        Cli::command().debug_assert();
    }

    #[test]
    fn serve_allows_github_by_default() {
        let cli = Cli::parse_from(["ix-credential", "serve"]);
        let Action::Serve { allow_host, .. } = cli.command else {
            panic!("expected serve");
        };
        assert_eq!(allow_host, vec![String::from("github.com")]);
    }

    #[test]
    fn a_remote_command_survives_the_double_dash() {
        let cli = Cli::parse_from([
            "ix-credential",
            "lend",
            "vin-compute-1",
            "--",
            "nix",
            "build",
            ".#thing",
        ]);
        let Action::Lend { remote_command, .. } = cli.command else {
            panic!("expected lend");
        };
        assert_eq!(remote_command, ["nix", "build", ".#thing"]);
    }
}
