//! Producer side: expose this process's terminals over a unix socket so an
//! out-of-process aggregator can render them.
//!
//! [`publish`] binds a [`UnixListener`] in the discovery directory
//! ([`socket_dir`](crate::socket_dir)) and streams a [`ProducerSnapshot`] as one
//! NDJSON line per poll tick to every connected reader. Each snapshot carries
//! the producer's full terminal set, so a late-joining aggregator needs no
//! backlog and the latest line fully describes this process.
//!
//! The producer holds no HTTP or CRDT dependency: it serializes frames and
//! writes bytes. The aggregator owns the Loro document and the browser-facing
//! server. This keeps every agent process that only *publishes* lightweight.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use tokio::io::AsyncWriteExt as _;
use tokio::net::{UnixListener, UnixStream};
use tokio::sync::watch;
use tokio::task::JoinHandle;

use crate::frame::{ProducerSnapshot, collect_frames};
use crate::{Error, Result, TuiManager};

fn publish_err(message: impl Into<String>) -> Error {
    Error::Publish {
        message: message.into(),
    }
}

/// A running producer. Dropping it, or calling [`Publisher::stop`], stops the
/// poll and accept loops and unlinks the socket file.
pub struct Publisher {
    path: PathBuf,
    producer: Arc<str>,
    shutdown: Option<watch::Sender<bool>>,
    tasks: Vec<JoinHandle<()>>,
}

impl Publisher {
    /// The socket path this producer is bound to.
    #[must_use]
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// This process's producer id, the scope its terminals appear under in the
    /// aggregated document.
    #[must_use]
    pub fn producer_id(&self) -> &str {
        &self.producer
    }

    /// Stop the loops, wait for them to wind down, and unlink the socket.
    /// Idempotent.
    pub async fn stop(&mut self) {
        let Some(shutdown) = self.shutdown.take() else {
            return;
        };
        let _ = shutdown.send(true);
        for task in self.tasks.drain(..) {
            task.abort();
            let _ = task.await;
        }
        let _ = std::fs::remove_file(&self.path);
    }
}

impl Drop for Publisher {
    /// Best-effort teardown for a handle that was never `stop`ped: signal the
    /// loops, abort them, and remove the socket so the aggregator stops listing
    /// a dead producer.
    fn drop(&mut self) {
        if let Some(shutdown) = self.shutdown.take() {
            let _ = shutdown.send(true);
        }
        for task in self.tasks.drain(..) {
            task.abort();
        }
        let _ = std::fs::remove_file(&self.path);
    }
}

/// Publish `manager`'s terminals on the unix socket at `path`.
///
/// `path` is usually [`socket_path`](crate::socket_path); a parent directory
/// this producer creates is mode `0700`, and a stale socket at `path` is reaped
/// first (a non-socket there is an error, never overwritten). `poll` is the
/// sampling interval; every tick writes the current [`ProducerSnapshot`] to all
/// connected readers.
///
/// The poll and accept loops run on the manager's runtime, so the producer
/// survives a temporary caller runtime being dropped.
pub async fn publish(
    manager: &Arc<TuiManager>,
    path: PathBuf,
    poll: Duration,
) -> Result<Publisher> {
    let producer: Arc<str> = Arc::from(
        format!(
            "{}-{}",
            std::process::id(),
            &uuid::Uuid::new_v4().simple().to_string()[..8]
        )
        .as_str(),
    );

    if let Some(parent) = path.parent() {
        // Only tighten permissions on a directory we create, never on a
        // caller-supplied directory that already exists (it could be `.` or
        // `$HOME`).
        let existed = parent.exists();
        std::fs::create_dir_all(parent)
            .map_err(|source| publish_err(format!("create {}: {source}", parent.display())))?;
        if !existed {
            restrict_dir(parent);
        }
    }
    reap_stale_socket(&path)?;

    let listener = UnixListener::bind(&path)
        .map_err(|source| publish_err(format!("bind {}: {source}", path.display())))?;
    restrict_socket(&path);

    let runtime = manager.runtime_handle();
    let initial = encode(&producer, manager).await;
    let (snapshot_tx, snapshot_rx) = watch::channel(initial);
    let (shutdown, _) = watch::channel(false);

    let poller = {
        let manager = manager.clone();
        let producer = producer.clone();
        let mut stop_rx = shutdown.subscribe();
        runtime.spawn(async move {
            loop {
                tokio::select! {
                    () = tokio::time::sleep(poll) => {}
                    _ = stop_rx.wait_for(|stop| *stop) => break,
                }
                let line = encode(&producer, &manager).await;
                if snapshot_tx.send(line).is_err() {
                    break;
                }
            }
        })
    };

    let accepter = {
        let mut stop_rx = shutdown.subscribe();
        // A separate receiver to hand each connection task, so cloning it does
        // not collide with the `wait_for` borrow of `stop_rx` in the select.
        let child_stop = shutdown.subscribe();
        runtime.spawn(async move {
            loop {
                tokio::select! {
                    accepted = listener.accept() => {
                        if let Ok((stream, _)) = accepted {
                            let rx = snapshot_rx.clone();
                            let stop = child_stop.clone();
                            tokio::spawn(write_loop(stream, rx, stop));
                        }
                    }
                    _ = stop_rx.wait_for(|stop| *stop) => break,
                }
            }
        })
    };

    Ok(Publisher {
        path,
        producer,
        shutdown: Some(shutdown),
        tasks: vec![poller, accepter],
    })
}

/// Serialize the manager's current terminals as one NDJSON line (trailing
/// newline included) for the producer's stream.
async fn encode(producer: &str, manager: &TuiManager) -> Arc<str> {
    let snapshot = ProducerSnapshot {
        producer: producer.to_owned(),
        terminals: collect_frames(manager).await,
    };
    // Serialization of this fixed shape cannot fail; fall back to an empty
    // terminal set rather than panicking a background task if it ever does.
    let body = serde_json::to_string(&snapshot).unwrap_or_else(|_| {
        format!("{{\"producer\":{producer:?},\"terminals\":[]}}")
    });
    Arc::from(format!("{body}\n").as_str())
}

/// Feed one connected reader: write the current snapshot, then each new one as
/// it lands, until the reader hangs up or the producer shuts down.
async fn write_loop(
    mut stream: UnixStream,
    mut rx: watch::Receiver<Arc<str>>,
    mut stop: watch::Receiver<bool>,
) {
    loop {
        let line = rx.borrow_and_update().clone();
        if stream.write_all(line.as_bytes()).await.is_err() {
            break;
        }
        tokio::select! {
            changed = rx.changed() => {
                if changed.is_err() {
                    break;
                }
            }
            _ = stop.wait_for(|s| *s) => break,
        }
    }
}

/// Reap a stale socket left by a crashed producer so `bind` does not fail with
/// `EADDRINUSE`.
///
/// Only an actual socket is removed. A path that exists and is something else (a
/// regular file or symlink from a caller-supplied path typo or reuse) is an
/// error, never silently deleted, so producing never clobbers real data.
fn reap_stale_socket(path: &Path) -> Result<()> {
    use std::os::unix::fs::FileTypeExt as _;
    match std::fs::symlink_metadata(path) {
        Ok(meta) if meta.file_type().is_socket() => std::fs::remove_file(path)
            .map_err(|source| publish_err(format!("remove stale socket {}: {source}", path.display()))),
        Ok(_) => Err(publish_err(format!(
            "{} exists and is not a socket; refusing to overwrite",
            path.display()
        ))),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(publish_err(format!("stat {}: {error}", path.display()))),
    }
}

/// Restrict the discovery directory to the owner (`0700`). Best-effort, applied
/// only to a directory this producer created.
fn restrict_dir(dir: &Path) {
    use std::os::unix::fs::PermissionsExt as _;
    let _ = std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700));
}

/// Restrict the socket to the owner (`0600`). Best-effort.
fn restrict_socket(path: &Path) {
    use std::os::unix::fs::PermissionsExt as _;
    let _ = std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600));
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A caller-supplied path that already holds a real file must never be
    /// deleted by the stale-socket reaper.
    #[test]
    fn reap_refuses_to_delete_a_non_socket() {
        let path =
            std::env::temp_dir().join(format!("ix-tui-reap-{}.notsock", std::process::id()));
        std::fs::write(&path, b"keep me").unwrap();

        let result = reap_stale_socket(&path);

        assert!(result.is_err(), "a regular file must not be reaped");
        assert!(path.exists(), "the file must survive a refused reap");
        std::fs::remove_file(&path).unwrap();
    }

    /// A missing path is nothing to reap, not an error.
    #[test]
    fn reap_is_ok_when_nothing_exists() {
        let path =
            std::env::temp_dir().join(format!("ix-tui-reap-missing-{}.sock", std::process::id()));
        let _ = std::fs::remove_file(&path);
        assert!(reap_stale_socket(&path).is_ok());
    }
}
