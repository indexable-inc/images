//! Consumer side: discover producer sockets and stream their snapshots.
//!
//! The mirror image of [`crate::publish`]. [`subscribe`] watches the discovery
//! directory ([`discovery_dir`](crate::discovery_dir)), connects to every
//! producer socket, parses each [`ProducerSnapshot`] NDJSON line, and forwards
//! it as a [`ProducerEvent`] on a channel. When a producer hangs up it emits a
//! [`ProducerEvent::Gone`] so the consumer can drop that producer's panes.
//!
//! The socket also carries a return direction. [`subscribe_bidi`] hands back
//! an [`InputRouter`] beside the event stream: each connection's write half is
//! registered under its producer id the moment the first snapshot names it,
//! and a routed [`InputLine`] reaches that producer as one NDJSON row (the
//! producer surfaces it via [`Publisher::inputs`](crate::Publisher::inputs)).
//!
//! Both consumers in the tree share this one implementation: the standalone
//! `dashboard` aggregator folds each event into its Loro [`Hub`](crate::Hub),
//! and `ix-windows` maps each event to native windows. Neither reimplements the
//! discovery/reaping logic.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use tokio::io::{AsyncBufReadExt as _, AsyncWriteExt as _, BufReader};
use tokio::net::UnixStream;
use tokio::net::unix::OwnedWriteHalf;
use tokio::runtime::Handle;
use tokio::sync::mpsc;

use crate::pane::{InputLine, ProducerSnapshot};

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

/// Depth of each connected producer's return-channel queue, mostly absorbing
/// a replay burst on (re)connect. See [`InputRouter::route`] for what happens
/// when it fills.
const ROUTE_DEPTH: usize = 256;

/// Routes viewer inputs back to connected producers: the return direction of
/// the producer socket.
///
/// [`read_producer`] registers each connection's write half here as soon as
/// its first snapshot line names the producer, and unregisters it when the
/// stream ends, so the map holds exactly the producers currently connected.
/// Cloneable; every clone shares one map.
#[derive(Clone, Default)]
pub struct InputRouter {
    /// Producer id to the sender feeding that connection's writer task.
    writers: Arc<Mutex<HashMap<String, mpsc::Sender<InputLine>>>>,
}

impl InputRouter {
    /// Queue `line` for `producer`, reporting whether it was queued.
    ///
    /// `false` is not an error to retry. It means the producer is not
    /// connected right now -- the consumer replays a producer's scoped inputs
    /// when it appears, which is what covers that gap -- or its queue is
    /// full. A full queue is a producer that stopped draining its return
    /// channel, and what is given up on is delivery to that wedged producer:
    /// awaiting space here would stall routing to every healthy one, and the
    /// missed value is restored by the replay on its next (re)connect.
    ///
    /// # Panics
    ///
    /// Panics if a thread panicked while holding the writer-map lock and
    /// poisoned it. Nothing this crate runs under that lock can panic, so a
    /// poisoned lock means a bug and not a state to limp on from.
    #[must_use]
    pub fn route(&self, producer: &str, line: InputLine) -> bool {
        let sender = self
            .writers
            .lock()
            .expect("router map poisoned")
            .get(producer)
            .cloned();
        let Some(sender) = sender else {
            return false;
        };
        sender.try_send(line).is_ok()
    }

    /// Hand `writer` a queue and record it under `producer`. A producer id
    /// re-registering (an aggregator-side reconnect to the same living
    /// producer) replaces the old queue; the old writer task ends when its
    /// closed channel drains.
    fn register(&self, producer: &str, writer: OwnedWriteHalf) {
        let (tx, rx) = mpsc::channel(ROUTE_DEPTH);
        self.writers
            .lock()
            .expect("router map poisoned")
            .insert(producer.to_owned(), tx);
        tokio::spawn(write_inputs(writer, rx));
    }

    /// Drop `producer`'s queue; its writer task ends with the closed channel.
    fn unregister(&self, producer: &str) {
        self.writers
            .lock()
            .expect("router map poisoned")
            .remove(producer);
    }
}

/// Feed one producer's return channel: serialize each routed [`InputLine`] as
/// one NDJSON row until the producer hangs up or the queue is unregistered.
async fn write_inputs(mut writer: OwnedWriteHalf, mut queue: mpsc::Receiver<InputLine>) {
    while let Some(line) = queue.recv().await {
        // An `InputLine` is strings all the way down, so this cannot fail;
        // skipping the row is still safer than panicking a background task.
        let Ok(body) = serde_json::to_string(&line) else {
            continue;
        };
        if writer
            .write_all(format!("{body}\n").as_bytes())
            .await
            .is_err()
        {
            break;
        }
    }
}

/// Both directions of a producer subscription, from [`subscribe_bidi`].
pub struct ProducerFeed {
    /// Producer snapshots and disconnects, exactly as [`subscribe`] yields
    /// them.
    pub events: mpsc::Receiver<ProducerEvent>,
    /// Routes viewer inputs back to whichever producers are connected.
    pub inputs: InputRouter,
}

/// Watch `dir` for producer sockets and stream their snapshots.
///
/// Spawns the discovery and per-socket read loops on `handle` and returns the
/// receiving end of a [`ProducerEvent`] channel. Each `*.sock` is read by
/// exactly one task; a re-created socket reconnects after its reader finishes.
/// Dropping the returned receiver winds the loops down: they observe the closed
/// channel on the next rescan or send and exit.
#[must_use]
pub fn subscribe(dir: PathBuf, rescan: Duration, handle: &Handle) -> mpsc::Receiver<ProducerEvent> {
    // The router is dropped: a read-only consumer (`ix-windows`) routes
    // nothing, and an unrouted registration is only a writer task idling
    // until its connection ends.
    subscribe_bidi(dir, rescan, handle).events
}

/// [`subscribe`], plus the return channel: the [`InputRouter`] half lets the
/// consumer write viewer inputs back to each connected producer over the same
/// socket its snapshots arrive on.
#[must_use]
pub fn subscribe_bidi(dir: PathBuf, rescan: Duration, handle: &Handle) -> ProducerFeed {
    let (tx, rx) = mpsc::channel(CHANNEL_DEPTH);
    let inputs = InputRouter::default();
    handle.spawn(discover(dir, rescan, tx, inputs.clone()));
    ProducerFeed { events: rx, inputs }
}

/// Rescan `dir` on a fixed interval and spawn a reader for each newly-seen
/// socket. `connected` is the set of sockets currently being read, so a socket
/// is read by exactly one task and a re-created socket reconnects after its
/// reader finishes.
async fn discover(
    dir: PathBuf,
    rescan: Duration,
    tx: mpsc::Sender<ProducerEvent>,
    router: InputRouter,
) {
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
                let router = router.clone();
                let connected = connected.clone();
                tokio::spawn(async move {
                    read_producer(&tx, &router, &path).await;
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
async fn read_producer(tx: &mpsc::Sender<ProducerEvent>, router: &InputRouter, path: &Path) {
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

    // The write half waits here until the first snapshot names the producer:
    // there is nothing to route to a producer that has not said who it is.
    // Registering before the event is sent means a consumer that reacts to a
    // first snapshot by replaying inputs cannot race the registration.
    let (reader, writer) = stream.into_split();
    let mut writer = Some(writer);
    let mut lines = BufReader::new(reader).lines();
    let mut producer_id: Option<String> = None;
    while let Ok(Some(line)) = lines.next_line().await {
        if line.is_empty() {
            continue;
        }
        // Skip a malformed line rather than dropping the producer: a future wire
        // version should degrade, not disconnect a working consumer.
        if let Ok(snapshot) = serde_json::from_str::<ProducerSnapshot>(&line) {
            if let Some(write_half) = writer.take() {
                router.register(&snapshot.producer, write_half);
            }
            producer_id = Some(snapshot.producer.clone());
            if tx.send(ProducerEvent::Snapshot(snapshot)).await.is_err() {
                break; // the consumer dropped the receiver; stop reading.
            }
        }
    }

    if let Some(producer) = producer_id {
        router.unregister(&producer);
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

    use super::{ProducerEvent, subscribe, subscribe_bidi};
    use crate::pane::{Input, InputLine, Pane};
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

    /// The return channel: an input routed to a connected producer arrives on
    /// its `Publisher::inputs` stream intact, and routing to a producer that
    /// is not connected reports so instead of silently vanishing.
    #[tokio::test(flavor = "multi_thread")]
    async fn routes_an_input_back_to_the_producer() {
        let dir = std::env::temp_dir().join(format!("ix-dash-bidi-{}", std::process::id()));
        let _ = std::fs::create_dir_all(&dir);
        let path = dir.join("p.sock");

        let mut publisher = Publisher::bind(path.clone(), &Handle::current()).expect("bind");
        let mut inputs = publisher.inputs().expect("first take");
        assert!(
            publisher.inputs().is_none(),
            "the inputs receiver is single-consumer"
        );
        let producer = publisher.producer_id().to_owned();
        publisher.publish(&[Pane::html("resource/x", "t", "<b>hi</b>")]);

        let mut feed = subscribe_bidi(dir.clone(), Duration::from_millis(20), &Handle::current());
        // The first snapshot event is sent after the write half is
        // registered, so from here the route below cannot miss.
        loop {
            match feed.events.recv().await.expect("event") {
                ProducerEvent::Snapshot(snapshot) if !snapshot.panes.is_empty() => break,
                _ => {}
            }
        }

        let line = InputLine {
            pane: "resource/x".to_owned(),
            field: "send".to_owned(),
            value: Input::Choice {
                value: r#"{"id":"u1","text":"hi"}"#.to_owned(),
            },
        };
        assert!(
            feed.inputs.route(&producer, line.clone()),
            "a connected producer must accept a routed input"
        );
        let got = tokio::time::timeout(Duration::from_secs(5), inputs.recv())
            .await
            .expect("the routed line must arrive")
            .expect("publisher alive");
        assert_eq!(got, line, "the line must survive the wire byte-exactly");

        assert!(
            !feed.inputs.route("nobody-connected", line),
            "routing to an unknown producer must say so"
        );

        publisher.stop().await;
        let _ = std::fs::remove_dir_all(&dir);
    }
}
