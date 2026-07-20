//! `ixterm`: client CLI for ix-term sessions (index#3797).
//!
//! `ixterm open <path>` asks the terminal server rendering this session to
//! open `<path>` by writing a private OSC sequence directly to the session's
//! pts in one `write(2)`. The payload is always a filesystem path, never
//! inline content, and there is no backchannel: the server renders errors in
//! the session UI.

mod osc;
mod pts;

use std::path::{Path, PathBuf};

use anyhow::Result;
use clap::{Parser, Subcommand};

#[derive(Parser)]
#[command(name = "ixterm", about = "ix-term session client")]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Ask the ix-term server to open a file in this session's UI.
    Open {
        /// File to open; canonicalized before sending, so it must exist here.
        path: PathBuf,
    },
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    match cli.command {
        Command::Open { path } => open(&path),
    }
}

#[cfg(target_os = "linux")]
fn open(path: &Path) -> Result<()> {
    use std::io::Write;

    use anyhow::{Context, ensure};

    let path = std::fs::canonicalize(path)
        .with_context(|| format!("cannot canonicalize {}", path.display()))?;
    let payload = osc::encode_open(&path)?;

    let resolution = pts::Resolution {
        session_id: std::env::var_os("IX_TERM_SESSION_ID"),
        sessions_root: PathBuf::from("/run/ix-term/sessions"),
        proc_root: PathBuf::from("/proc"),
        start_pid: std::os::unix::process::parent_id(),
    };
    let pts = pts::resolve(&resolution)?;

    let mut device = std::fs::OpenOptions::new()
        .write(true)
        .open(&pts)
        .with_context(|| format!("cannot open {} for writing", pts.display()))?;

    // A single write(2) keeps the escape sequence contiguous: interleaving
    // with other writers on the pts would corrupt the OSC framing.
    let written = device
        .write(&payload)
        .with_context(|| format!("write to {} failed", pts.display()))?;
    ensure!(
        written == payload.len(),
        "partial write to {}: {written} of {} bytes",
        pts.display(),
        payload.len(),
    );
    Ok(())
}

#[cfg(not(target_os = "linux"))]
fn open(path: &Path) -> Result<()> {
    anyhow::bail!(
        "ixterm open is linux-only (it resolves the session pts via /proc and /dev/pts); \
         cannot open {}",
        path.display(),
    )
}
