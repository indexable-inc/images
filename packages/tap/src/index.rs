//! The session index: a JSON list of live sessions under the runtime dir.
//!
//! Writers take an exclusive file lock so two daemons starting at once cannot
//! lose each other's record. Readers never trust a record blindly: a session
//! counts as live only if its socket still exists and its daemon PID is alive,
//! so a crashed daemon's stale entry is filtered out instead of shown or dialed.

use std::io::{Read as _, Seek as _, Write as _};

use anyhow::{Context as _, Result};
use tap_protocol::Session;

/// Apply `f` to the session list under an exclusive lock, then persist it.
fn modify(f: impl FnOnce(&mut Vec<Session>)) -> Result<()> {
    let path = tap_protocol::sessions_file();
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("creating runtime dir {}", parent.display()))?;
    }

    let mut file = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(&path)
        .with_context(|| format!("opening session index {}", path.display()))?;
    file.lock().context("locking session index")?;

    let mut content = String::new();
    file.read_to_string(&mut content)
        .context("reading session index")?;
    let mut sessions: Vec<Session> = if content.trim().is_empty() {
        Vec::new()
    } else {
        serde_json::from_str(&content).unwrap_or_default()
    };

    f(&mut sessions);

    let rendered = serde_json::to_string_pretty(&sessions).context("serializing session index")?;
    file.set_len(0).context("truncating session index")?;
    file.seek(std::io::SeekFrom::Start(0))
        .context("rewinding session index")?;
    file.write_all(rendered.as_bytes())
        .context("writing session index")?;
    Ok(())
}

/// Record a session, replacing any stale entry with the same id.
///
/// # Errors
///
/// Returns an error if the index cannot be read or written.
pub fn add(session: Session) -> Result<()> {
    modify(|sessions| {
        sessions.retain(|s| s.id != session.id);
        sessions.push(session);
    })
}

/// Remove a session by id.
///
/// # Errors
///
/// Returns an error if the index cannot be read or written.
pub fn remove(id: &str) -> Result<()> {
    modify(|sessions| sessions.retain(|s| s.id != id))
}

/// All recorded sessions, including any stale ones.
fn read_all() -> Vec<Session> {
    let content = std::fs::read_to_string(tap_protocol::sessions_file()).unwrap_or_default();
    if content.trim().is_empty() {
        return Vec::new();
    }
    serde_json::from_str(&content).unwrap_or_default()
}

/// Whether a process is still alive (`kill(pid, 0)`).
#[must_use]
pub fn pid_alive(pid: u32) -> bool {
    let Ok(pid) = i32::try_from(pid) else {
        return false;
    };
    if pid <= 0 {
        return false;
    }
    // 0 => signalable, EPERM => exists but not ours, ESRCH => gone.
    if unsafe { nix::libc::kill(pid, 0) } == 0 {
        return true;
    }
    std::io::Error::last_os_error().raw_os_error() == Some(nix::libc::EPERM)
}

/// Sessions whose socket exists and whose daemon is still running.
#[must_use]
pub fn list_live() -> Vec<Session> {
    read_all()
        .into_iter()
        .filter(|s| s.socket.exists() && pid_alive(s.pid))
        .collect()
}

/// The most recently started live session, if any.
#[must_use]
pub fn latest_live() -> Option<Session> {
    let mut live = list_live();
    live.sort_by_key(|s| s.started_unix);
    live.pop()
}
