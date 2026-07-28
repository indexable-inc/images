//! Consumer side: discover producer sockets and stream their snapshots.
//!
//! The mirror image of [`crate::publish`]. [`subscribe`] watches the discovery
//! directory ([`discovery_dir`](crate::discovery_dir)), connects to every
//! producer socket, parses each [`ProducerSnapshot`] NDJSON line, and forwards
//! it as a [`ProducerEvent`] on a channel. When a producer hangs up it emits a
//! [`ProducerEvent::Gone`] so the consumer can drop that producer's panes.
//!
//! Both consumers in the tree share this one implementation: the standalone
//! `dashboard` aggregator folds each event into its Loro [`Hub`](crate::Hub),
//! and `ix-windows` maps each event to native windows. Neither reimplements the
//! discovery/reaping logic.

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use tokio::io::{AsyncBufReadExt as _, BufReader};
use tokio::net::UnixStream;
use tokio::runtime::Handle;
use tokio::sync::mpsc;

use crate::pane::ProducerSnapshot;

/// An event from the producer fleet.
#[derive(Debug, Clone)]
pub enum ProducerEvent {
    /// A producer's current full pane set (replacement semantics: the latest
    /// snapshot fully describes that producer).
    Snapshot(ProducerSnapshot),
    /// A producer disconnected; everything under its scope should leave the
    /// consumer's view.
    Gone {
        /// The producer id whose panes are now gone.
        producer: String,
    },
}

/// Channel depth for the event stream. Reader tasks `await` the send, so a slow
/// consumer applies backpressure rather than dropping snapshots; a small buffer
/// is enough to absorb a burst of producers appearing at once.
const CHANNEL_DEPTH: usize = 256;

/// Watch `dir` for producer sockets and stream their snapshots.
///
/// Spawns the discovery and per-socket read loops on `handle` and returns the
/// receiving end of a [`ProducerEvent`] channel. Each `*.sock` is read by
/// exactly one task; a re-created socket reconnects after its reader finishes.
/// Dropping the returned receiver winds the loops down: they observe the closed
/// channel on the next rescan or send and exit.
#[must_use]
pub fn subscribe(dir: PathBuf, rescan: Duration, handle: &Handle) -> mpsc::Receiver<ProducerEvent> {
    let (tx, rx) = mpsc::channel(CHANNEL_DEPTH);
    handle.spawn(discover(dir, rescan, tx));
    rx
}

/// Rescan `dir` on a fixed interval and spawn a reader for each newly-seen
/// socket. `connected` is the set of sockets currently being read, so a socket
/// is read by exactly one task and a re-created socket reconnects after its
/// reader finishes.
async fn discover(dir: PathBuf, rescan: Duration, tx: mpsc::Sender<ProducerEvent>) {
    let connected: Arc<Mutex<HashSet<PathBuf>>> = Arc::new(Mutex::new(HashSet::new()));
    loop {
        if tx.is_closed() {
            return;
        }
        if let Ok(entries) = std::fs::read_dir(&dir) {
            for entry in entries.flatten() {
                let path = entry.path();
                if path.extension().and_then(|ext| ext.to_str()) != Some("sock") {
                    continue;
                }
                if !connected
                    .lock()
                    .expect("connected set poisoned")
                    .insert(path.clone())
                {
                    continue;
                }
                let tx = tx.clone();
                let connected = connected.clone();
                tokio::spawn(async move {
                    read_producer(&tx, &path).await;
                    connected
                        .lock()
                        .expect("connected set poisoned")
                        .remove(&path);
                });
            }
        }
        tokio::time::sleep(rescan).await;
    }
}

/// Connect to one producer socket and forward its NDJSON stream as
/// [`ProducerEvent::Snapshot`]s until the producer hangs up, then emit one
/// [`ProducerEvent::Gone`]. A stale socket file (connection refused) is reaped.
async fn read_producer(tx: &mpsc::Sender<ProducerEvent>, path: &Path) {
    let stream = match UnixStream::connect(path).await {
        Ok(stream) => stream,
        Err(error) => {
            // A bound, listening socket accepts immediately, so a refusal means
            // the socket file outlived its producer. Reap it, but only if it is
            // actually a socket: a regular `*.sock` file a user dropped in the
            // watched directory also refuses, and must not be deleted.
            if error.kind() == std::io::ErrorKind::ConnectionRefused && is_socket(path) {
                let _ = std::fs::remove_file(path);
            }
            return;
        }
    };

    let mut lines = BufReader::new(stream).lines();
    let mut producer_id: Option<String> = None;
    while let Ok(Some(line)) = lines.next_line().await {
        if line.is_empty() {
            continue;
        }
        // Skip a malformed line rather than dropping the producer: a future wire
        // version should degrade, not disconnect a working consumer.
        if let Ok(snapshot) = serde_json::from_str::<ProducerSnapshot>(&line) {
            producer_id = Some(snapshot.producer.clone());
            if tx.send(ProducerEvent::Snapshot(snapshot)).await.is_err() {
                return; // the consumer dropped the receiver; stop reading.
            }
        }
    }

    if let Some(producer) = producer_id {
        let _ = tx.send(ProducerEvent::Gone { producer }).await;
    }
}

/// Whether `path` is a unix socket, used to avoid reaping a regular file that a
/// user happened to name `*.sock` in the watched directory.
fn is_socket(path: &Path) -> bool {
    use std::os::unix::fs::FileTypeExt as _;
    std::fs::symlink_metadata(path).is_ok_and(|meta| meta.file_type().is_socket())
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use tokio::runtime::Handle;

    use super::{ProducerEvent, subscribe};
    use crate::pane::Pane;
    use crate::publish::Publisher;

    /// A producer that binds, publishes one pane, then drops yields a `Snapshot`
    /// carrying that pane followed by a `Gone` for the same producer id.
    #[tokio::test(flavor = "multi_thread")]
    async fn streams_snapshot_then_gone_on_disconnect() {
        let dir = std::env::temp_dir().join(format!("ix-dash-sub-{}", std::process::id()));
        let _ = std::fs::create_dir_all(&dir);
        let path = dir.join("p.sock");

        let mut publisher = Publisher::bind(path.clone(), &Handle::current()).expect("bind");
        let producer = publisher.producer_id().to_owned();
        publisher.publish(&[Pane::html("resource/x", "t", "<b>hi</b>")]);

        let mut rx = subscribe(dir.clone(), Duration::from_millis(20), &Handle::current());

        // First non-empty snapshot must carry the published pane.
        let snapshot = loop {
            match rx.recv().await.expect("event") {
                ProducerEvent::Snapshot(s) if !s.panes.is_empty() => break s,
                _ => {}
            }
        };
        assert_eq!(snapshot.producer, producer);
        assert_eq!(snapshot.panes[0].id, "resource/x");

        // Dropping the publisher unlinks the socket and closes the stream, so the
        // reader emits a `Gone` for this producer.
        publisher.stop().await;
        let gone = loop {
            match rx.recv().await.expect("event") {
                ProducerEvent::Gone { producer } => break producer,
                ProducerEvent::Snapshot(_) => {}
            }
        };
        assert_eq!(gone, producer);

        let _ = std::fs::remove_dir_all(&dir);
    }
}
