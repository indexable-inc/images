//! `chromium-cookies` - pull decrypted cookies out of any macOS Chromium app.
//!
//! Built to sync a local browser session onto a remote ix VM: extract here as
//! JSON or a Netscape `cookies.txt`, ship the file over, and replant it in the
//! VM's browser or an HTTP client. `list` discovers installed apps.

mod browser;
mod crypto;
mod keychain;
mod store;

use anyhow::{Context, Result};
use clap::{Parser, Subcommand, ValueEnum};
use std::fmt::Write as _;
use store::Cookie;

/// Seconds between the Chromium epoch (1601-01-01) and the Unix epoch.
const CHROMIUM_TO_UNIX_SECS: i64 = 11_644_473_600;

#[derive(Parser)]
#[command(about, version)]
struct Cli {
    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Subcommand)]
enum Cmd {
    /// List Chromium apps on this machine that have a cookie store.
    List,
    /// Extract and decrypt cookies for one app (e.g. `dia`, `chrome`, `Slack`).
    Extract {
        /// App display name or `Application Support` subdirectory.
        app: String,
        /// Output format.
        #[arg(long, value_enum, default_value_t = Format::Json)]
        format: Format,
        /// Keep only cookies whose host contains this substring.
        #[arg(long)]
        domain: Option<String>,
    },
}

#[derive(Copy, Clone, ValueEnum)]
enum Format {
    Json,
    Netscape,
}

fn main() -> Result<()> {
    match Cli::parse().cmd {
        Cmd::List => list(),
        Cmd::Extract {
            app,
            format,
            domain,
        } => extract(&app, format, domain.as_deref()),
    }
}

fn list() -> Result<()> {
    for t in browser::discover()? {
        println!("{:<24} {}", t.name, t.cookie_db.display());
    }
    Ok(())
}

fn extract(app: &str, format: Format, domain: Option<&str>) -> Result<()> {
    let target = browser::resolve(app)?;
    let password = keychain::safe_storage_password(&target.keychain_service)
        .with_context(|| format!("reading Keychain secret {:?}", target.keychain_service))?;
    let key = crypto::derive_key(&password);

    let mut cookies =
        store::read(&target.cookie_db, &key).with_context(|| format!("reading {}", target.cookie_db.display()))?;
    if let Some(d) = domain {
        cookies.retain(|c| c.host.contains(d));
    }

    match format {
        Format::Json => println!("{}", serde_json::to_string_pretty(&cookies)?),
        Format::Netscape => print!("{}", netscape(&cookies)),
    }
    eprintln!("{} cookies", cookies.len());
    Ok(())
}

/// Render cookies as a Netscape `cookies.txt`, the format curl and browsers
/// import. Chromium expiry (microseconds since 1601) becomes Unix seconds.
fn netscape(cookies: &[Cookie]) -> String {
    let mut out = String::from("# Netscape HTTP Cookie File\n");
    for c in cookies {
        let expires = if c.expires_utc == 0 {
            0
        } else {
            c.expires_utc / 1_000_000 - CHROMIUM_TO_UNIX_SECS
        };
        let subdomains = if c.host.starts_with('.') { "TRUE" } else { "FALSE" };
        let secure = if c.secure { "TRUE" } else { "FALSE" };
        // Infallible: writing to a String never errors.
        let _ = writeln!(
            out,
            "{}\t{subdomains}\t{}\t{secure}\t{expires}\t{}\t{}",
            c.host, c.path, c.name, c.value
        );
    }
    out
}
