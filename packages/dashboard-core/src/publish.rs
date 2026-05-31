//! Producer side: expose this process's panes over a unix socket so the
//! out-of-process aggregator can render them.
//!
//! [`Publisher::bind`] binds a [`UnixListener`] in the discovery directory
//! ([`discovery_dir`](crate::discovery_dir)) and streams a [`ProducerSnapshot`]
//! as one NDJSON line to every connected reader. [`Publisher::publish`] (or a
//! cloned [`PaneSink`] handed to a background loop) replaces the streamed
//! snapshot, so the latest line fully describes this process and a late-joining
//! aggregator needs no backlog.
//!
//! The producer holds no HTTP or CRDT dependency: it serializes panes and writes
//! bytes. The aggregator owns the Loro document and the browser-facing server.
//! This keeps every process that only *publishes* lightweight, and any
//! producer — a terminal manager, a VM controller, a future resource — drives
//! the same socket with its own [`Pane`](crate::Pane) list.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use tokio::io::AsyncWriteExt as _;
use tokio::net::{UnixListener, UnixStream};
use tokio::sync::watch;
use tokio::task::JoinHandle;

use crate::pane::{Pane, ProducerSnapshot};
use crate::{Error, Result};

fn publish_err(message: impl Into<String>) -> Error {
    Error::Dashboard {
        message: format!("publish: {}", message.into()),
    }
}

/// A short, unique per-process producer id: `"<pid>-<short-uuid>"`.
fn new_producer_id() -> Arc<str> {
    Arc::from(
        format!(
            "{}-{}",
            std::process::id(),
            &uuid::Uuid::new_v4().simple().to_string()[..8]
        )
        .as_str(),
    )
}

/// A cloneable handle for pushing pane snapshots into a [`Publisher`].
///
/// A producer that samples its resource on a background task (a poll loop over a
/// terminal manager, an event handler on a VM) holds a `PaneSink` cloned from
/// [`Publisher::sink`] and calls [`publish`](Self::publish) whenever the set
/// changes. The socket and the readers stay with the owning [`Publisher`].
#[derive(Clone)]
pub struct PaneSink {
    producer: Arc<str>,
    // `Arc` so many background tasks can share one watch sender; `watch::Sender`
    // is not itself `Clone`, but `send` takes `&self`, so sharing it is enough.
    snapshot_tx: Arc<watch::Sender<Arc<str>>>,
}

impl PaneSink {
    /// Replace the snapshot streamed to every connected reader with `panes`.
    ///
    /// Cheap and synchronous: it serializes one NDJSON line and stores it. A
    /// reader that is mid-write picks up the new line on its next turn.
    pub fn publish(&self, panes: &[Pane]) {
        let line = encode(&self.producer, panes);
        // The only receivers are the per-connection write loops; if all readers
        // have hung up the send still updates the stored value for the next one.
        let _ = self.snapshot_tx.send(line);
    }

    /// This process's producer id, the scope its panes appear under in the
    /// aggregated document.
    #[must_use]
    pub fn producer_id(&self) -> &str {
        &self.producer
    }
}

/// A running producer. Dropping it, or calling [`Publisher::stop`], stops the
/// accept loop and any attached task and unlinks the socket file.
///
/// The caller drives the content via [`publish`](Self::publish) or a cloned
/// [`sink`](Self::sink); the publisher only owns the socket and the fan-out.
pub struct Publisher {
    path: PathBuf,
    sink: PaneSink,
    shutdown: Option<watch::Sender<bool>>,
    tasks: Vec<JoinHandle<()>>,
}

impl Publisher {
    /// Bind a producer socket at `path` and start accepting readers on `runtime`.
    ///
    /// `path` is usually [`socket_path`](crate::socket_path); a parent directory
    /// this producer creates is mode `0700`, and a stale socket at `path` is
    /// reaped first (a non-socket there is an error, never overwritten). The
    /// accept loop runs on `runtime`, so the producer survives a temporary
    /// caller runtime being dropped; pass `&Handle::current()` to use the
    /// ambient one.
    ///
    /// The producer starts with an empty pane set; call [`publish`](Self::publish)
    /// to populate it.
    ///
    /// # Errors
    ///
    /// Returns [`Error::Dashboard`] when the directory cannot be created or the
    /// socket cannot be bound.
    pub fn bind(path: PathBuf, runtime: &tokio::runtime::Handle) -> Result<Self> {
        let producer = new_producer_id();

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

        // `UnixListener::bind` registers the socket with the IO driver, which
        // requires an active runtime context on the *calling* thread. The whole
        // point of taking a `Handle` is to let a caller bind from a thread that
        // is not itself a runtime worker (e.g. one driving a native run loop), so
        // enter the handle's context for the bind. Re-entering is harmless when
        // the caller is already inside this runtime.
        let listener = {
            let _enter = runtime.enter();
            UnixListener::bind(&path)
                .map_err(|source| publish_err(format!("bind {}: {source}", path.display())))?
        };
        restrict_socket(&path);

        let initial = encode(&producer, &[]);
        let (snapshot_tx, snapshot_rx) = watch::channel(initial);
        let (shutdown, _) = watch::channel(false);

        let accepter = {
            let mut stop_rx = shutdown.subscribe();
            // A separate receiver to hand each connection task, so cloning it
            // does not collide with the `wait_for` borrow of `stop_rx` in the
            // select.
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

        Ok(Self {
            path,
            sink: PaneSink {
                producer,
                snapshot_tx: Arc::new(snapshot_tx),
            },
            shutdown: Some(shutdown),
            tasks: vec![accepter],
        })
    }

    /// Replace the snapshot streamed to every connected reader with `panes`.
    pub fn publish(&self, panes: &[Pane]) {
        self.sink.publish(panes);
    }

    /// A cloneable sink for pushing snapshots from a background task. Its
    /// lifetime is independent of the `Publisher`; attach the task with
    /// [`push_task`](Self::push_task) so it stops with the publisher.
    #[must_use]
    pub fn sink(&self) -> PaneSink {
        self.sink.clone()
    }

    /// Attach a producer task (a sampling loop) whose lifetime is tied to this
    /// publisher, so [`stop`](Self::stop) and `Drop` wind it down with the
    /// accept loop.
    pub fn push_task(&mut self, task: JoinHandle<()>) {
        self.tasks.push(task);
    }

    /// This process's producer id, the scope its panes appear under in the
    /// aggregated document.
    #[must_use]
    pub fn producer_id(&self) -> &str {
        self.sink.producer_id()
    }

    /// The socket path this producer is bound to.
    #[must_use]
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Stop the accept loop and every attached task, wait for them to wind down,
    /// and unlink the socket. Idempotent.
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
    /// loops, abort them, and remove the socket so the aggregator stops listing a
    /// dead producer.
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

/// Serialize a pane set as one NDJSON line (trailing newline included).
fn encode(producer: &str, panes: &[Pane]) -> Arc<str> {
    let snapshot = ProducerSnapshot {
        producer: producer.to_owned(),
        panes: panes.to_vec(),
    };
    // A pane carries arbitrary JSON in a data view, which cannot fail to
    // serialize here; fall back to an empty pane set rather than panicking a
    // background task if it ever does.
    let body = serde_json::to_string(&snapshot)
        .unwrap_or_else(|_| format!("{{\"producer\":{producer:?},\"panes\":[]}}"));
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
        Ok(meta) if meta.file_type().is_socket() => std::fs::remove_file(path).map_err(|source| {
            publish_err(format!("remove stale socket {}: {source}", path.display()))
        }),
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
    use super::reap_stale_socket;

    /// A caller-supplied path that already holds a real file must never be
    /// deleted by the stale-socket reaper.
    #[test]
    fn reap_refuses_to_delete_a_non_socket() {
        let path =
            std::env::temp_dir().join(format!("ix-dash-reap-{}.notsock", std::process::id()));
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
            std::env::temp_dir().join(format!("ix-dash-reap-missing-{}.sock", std::process::id()));
        let _ = std::fs::remove_file(&path);
        assert!(reap_stale_socket(&path).is_ok());
    }
}
