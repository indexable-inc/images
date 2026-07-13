//! Peer-to-peer session networking: score gossip, blob transfer, and the
//! shared clock, over plain TCP + UDP on the LAN.
//!
//! Composability seams:
//! - Peer discovery is *injected*: [`Config::peers`] is a static list, so
//!   sandboxed tests and simple deployments need no multicast; an mDNS
//!   discoverer can feed the same list later.
//! - Time is injected via [`MonotonicTime`], so tests can drive the clock.
//! - The score and blob store are shared handles ([`std::sync::Arc`]); the
//!   node gossips whatever they contain and never interprets audio.
//!
//! Leadership: the peer with the smallest [`PeerId`] leads; everyone else
//! follows its epoch and answers its pings. Every peer runs the same code,
//! so leadership needs no coordination beyond comparing ids in `Hello`.

pub mod wire;

use std::collections::BTreeMap;
use std::net::SocketAddr;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use anyhow::{Context as _, Result};
use audio_blob::BlobStore;
use audio_clock::{MonotonicTime, OffsetEstimator, PeerId, PingSample, SharedClock};
use audio_score::Score;
use tokio::net::{TcpListener, TcpStream, UdpSocket};
use tokio::sync::watch;
use tracing::{debug, warn};

use wire::{Hello, Message, Ping};

/// How often peers gossip score deltas and clock pings.
const GOSSIP_INTERVAL: Duration = Duration::from_millis(250);
const PING_INTERVAL: Duration = Duration::from_millis(500);

/// Everything a node needs to join a session.
pub struct Config {
    /// This node's stable identity; smallest id in the session leads.
    pub peer_id: PeerId,
    /// TCP address to listen on (`127.0.0.1:0` for an ephemeral port).
    pub tcp_bind: SocketAddr,
    /// UDP address for clock pings (`127.0.0.1:0` for an ephemeral port).
    pub udp_bind: SocketAddr,
    /// Known peers to dial (their TCP addresses). Injected, not discovered.
    pub peers: Vec<SocketAddr>,
    /// Session sample rate advertised in `Hello`.
    pub sample_rate: u32,
    /// Time source; [`audio_clock::ProcessTime`] outside tests.
    pub time: Arc<dyn MonotonicTime>,
}

/// A live node; dropping the handle stops its tasks.
pub struct NodeHandle {
    /// The address the TCP listener actually bound (resolves `:0`).
    pub tcp_addr: SocketAddr,
    /// The address the UDP socket actually bound.
    pub udp_addr: SocketAddr,
    clock: watch::Receiver<SharedClock>,
    tasks: Vec<tokio::task::JoinHandle<()>>,
    /// Per-connection tasks spawned by the accept loop; aborted on drop so
    /// a stopped node cannot keep gossiping (or keep its sockets alive).
    conn_tasks: Arc<Mutex<Vec<tokio::task::JoinHandle<()>>>>,
}

impl NodeHandle {
    /// The current shared-clock snapshot (copy; cheap).
    #[must_use]
    pub fn clock(&self) -> SharedClock {
        *self.clock.borrow()
    }

    /// Wait until the clock changes (a follower adopted the leader's epoch).
    ///
    /// # Errors
    /// Fails when the node's clock task has stopped.
    pub async fn clock_changed(&mut self) -> Result<SharedClock> {
        self.clock.changed().await.context("node clock task stopped")?;
        Ok(*self.clock.borrow())
    }
}

impl Drop for NodeHandle {
    fn drop(&mut self) {
        for task in &self.tasks {
            task.abort();
        }
        for task in self.conn_tasks.lock().expect("conn tasks lock").drain(..) {
            task.abort();
        }
    }
}

/// Shared state every task reads.
struct Node {
    config: Config,
    score: Arc<Mutex<Score>>,
    store: Arc<BlobStore>,
    clock: watch::Sender<SharedClock>,
    estimator: Mutex<OffsetEstimator>,
    /// The leader we currently follow, if any (smaller id than ours).
    leader: Mutex<Option<Leader>>,
    /// Every peer with a live connection, keyed by id; the election in
    /// [`Node::elect`] follows the smallest one, and a peer leaving
    /// (connection count hitting zero) triggers a re-election.
    peers: Mutex<BTreeMap<PeerId, PeerEntry>>,
    udp: UdpSocket,
}

#[derive(Debug, Clone, Copy)]
struct Leader {
    peer_id: PeerId,
    epoch_micros: i64,
    ping_addr: SocketAddr,
}

#[derive(Debug, Clone, Copy)]
struct PeerEntry {
    ping_addr: SocketAddr,
    epoch_micros: i64,
    /// Live connections carrying this peer (we may both dial and accept).
    connections: usize,
}

/// Spawn a node: listen, dial `config.peers`, gossip, and serve pings.
///
/// # Errors
/// Fails when the sockets cannot bind.
pub async fn spawn(
    config: Config,
    score: Arc<Mutex<Score>>,
    blobs: Arc<BlobStore>,
) -> Result<NodeHandle> {
    let listener = TcpListener::bind(config.tcp_bind).await.context("bind TCP")?;
    let tcp_addr = listener.local_addr()?;
    let udp = UdpSocket::bind(config.udp_bind).await.context("bind UDP")?;
    let udp_addr = udp.local_addr()?;

    // Until a smaller peer id appears, this node leads on its own timeline.
    let now = config.time.now_micros();
    let (clock_tx, clock_rx) = watch::channel(SharedClock::lead(now));

    let node = Arc::new(Node {
        config,
        score,
        store: blobs,
        clock: clock_tx,
        estimator: Mutex::new(OffsetEstimator::new(16)),
        leader: Mutex::new(None),
        peers: Mutex::new(BTreeMap::new()),
        udp,
    });

    let conn_tasks = Arc::new(Mutex::new(Vec::new()));
    let mut tasks = Vec::new();
    tasks.push(tokio::spawn(accept_loop(
        Arc::clone(&node),
        listener,
        Arc::clone(&conn_tasks),
    )));
    for peer in node.config.peers.clone() {
        tasks.push(tokio::spawn(dial_loop(Arc::clone(&node), peer)));
    }
    tasks.push(tokio::spawn(ping_loop(Arc::clone(&node))));

    Ok(NodeHandle { tcp_addr, udp_addr, clock: clock_rx, tasks, conn_tasks })
}

async fn accept_loop(
    node: Arc<Node>,
    listener: TcpListener,
    conn_tasks: Arc<Mutex<Vec<tokio::task::JoinHandle<()>>>>,
) {
    loop {
        match listener.accept().await {
            Ok((stream, addr)) => {
                debug!(%addr, "peer connected");
                let task = tokio::spawn(serve_connection(Arc::clone(&node), stream));
                let mut tasks = conn_tasks.lock().expect("conn tasks lock");
                tasks.retain(|task| !task.is_finished());
                tasks.push(task);
            }
            Err(error) => {
                warn!(%error, "accept failed");
                tokio::time::sleep(GOSSIP_INTERVAL).await;
            }
        }
    }
}

/// Keep one outbound connection alive to a configured peer.
async fn dial_loop(node: Arc<Node>, peer: SocketAddr) {
    loop {
        match TcpStream::connect(peer).await {
            Ok(stream) => {
                debug!(%peer, "dialed peer");
                if let Err(error) = drive_connection(Arc::clone(&node), stream, peer).await {
                    warn!(%peer, %error, "peer connection ended");
                }
            }
            Err(error) => debug!(%peer, %error, "dial failed; will retry"),
        }
        tokio::time::sleep(GOSSIP_INTERVAL).await;
    }
}

async fn serve_connection(node: Arc<Node>, stream: TcpStream) {
    let peer = stream.peer_addr().unwrap_or_else(|_| ([0, 0, 0, 0], 0).into());
    if let Err(error) = drive_connection(node, stream, peer).await {
        warn!(%peer, %error, "peer connection ended");
    }
}

/// The symmetric per-connection protocol: exchange `Hello`, then
/// periodically push score deltas the remote is missing and answer blob
/// requests. Both sides run this same loop.
async fn drive_connection(node: Arc<Node>, mut stream: TcpStream, peer: SocketAddr) -> Result<()> {
    let hello = Message::Hello(Hello {
        peer_id: node.config.peer_id.0,
        udp_port: node.udp.local_addr()?.port(),
        // The epoch on *our* local clock: peers ping this node's clock, so
        // a follower must translate the leader's epoch, not parrot it.
        epoch_micros: node.clock.borrow().local_epoch_micros(),
        sample_rate: node.config.sample_rate,
    });
    hello.write_to(&mut stream).await?;

    let first = Message::read_from(&mut stream)
        .await?
        .context("peer closed before Hello")?;
    let Message::Hello(remote) = first else {
        anyhow::bail!("peer spoke before Hello");
    };
    node.peer_connected(&remote, peer);
    let result = gossip_loop(&node, &mut stream).await;
    node.peer_disconnected(PeerId(remote.peer_id));
    result
}

/// The steady-state half of [`drive_connection`], bracketed by peer
/// registration so a vanished peer always triggers a re-election.
async fn gossip_loop(node: &Node, stream: &mut TcpStream) -> Result<()> {
    // Track what the remote has seen; start from empty so the first gossip
    // tick sends the full history (cheap: scores are tiny).
    let mut sent = audio_score::VersionVector::new();
    let mut ticker = tokio::time::interval(GOSSIP_INTERVAL);
    loop {
        tokio::select! {
            _ = ticker.tick() => {
                let update = {
                    let score = node.score.lock().expect("score lock");
                    let current = score.version();
                    if current == sent {
                        None
                    } else {
                        let update = score.export_updates(&sent)?;
                        drop(score);
                        sent = current;
                        Some(update)
                    }
                };
                if let Some(update) = update {
                    Message::ScoreUpdate(update).write_to(&mut *stream).await?;
                }
                // Fetch the instrument blob as soon as the score names one
                // we do not hold.
                let missing = {
                    let score = node.score.lock().expect("score lock");
                    score.instrument().ok().flatten().map(|instrument| instrument.hash)
                };
                if let Some(hash) = missing
                    && !node.store.contains(&hash)
                {
                    Message::BlobRequest(hash).write_to(&mut *stream).await?;
                }
            }
            message = Message::read_from(&mut *stream) => {
                let Some(message) = message? else { return Ok(()) };
                node.apply(message, &mut *stream).await?;
            }
        }
    }
}

impl Node {
    /// Handle one inbound message on a connection.
    async fn apply(&self, message: Message, stream: &mut TcpStream) -> Result<()> {
        match message {
            Message::Hello(_) => {} // duplicate Hello; harmless
            Message::ScoreUpdate(update) => {
                let score = self.score.lock().expect("score lock");
                if let Err(error) = score.import(&update) {
                    warn!(%error, "dropping malformed score update");
                }
            }
            Message::BlobRequest(hash) => {
                if let Ok(Some(bytes)) = self.store.get(&hash) {
                    Message::Blob(hash, bytes).write_to(stream).await?;
                }
            }
            Message::Blob(hash, bytes) => {
                // `put` re-derives the hash, so a lying peer cannot poison
                // the store; a mismatch just stores under the true hash and
                // the request stays outstanding.
                let stored = audio_blob::BlobHash::of(&bytes);
                if stored == hash {
                    self.store.put(&bytes)?;
                } else {
                    warn!(claimed = %hash, actual = %stored, "peer sent mismatched blob");
                }
            }
        }
        Ok(())
    }

    /// Register a peer whose `Hello` arrived on a live connection, then
    /// re-run the election.
    fn peer_connected(&self, remote: &Hello, tcp_addr: SocketAddr) {
        let remote_id = PeerId(remote.peer_id);
        let mut peers = self.peers.lock().expect("peers lock");
        peers
            .entry(remote_id)
            .or_insert_with(|| PeerEntry {
                ping_addr: SocketAddr::new(tcp_addr.ip(), remote.udp_port),
                epoch_micros: remote.epoch_micros,
                connections: 0,
            })
            .connections += 1;
        drop(peers);
        self.elect();
    }

    /// Drop one connection's claim on a peer; when the last one goes, the
    /// peer has left the session and the election reruns.
    fn peer_disconnected(&self, remote_id: PeerId) {
        {
            let mut peers = self.peers.lock().expect("peers lock");
            if let Some(entry) = peers.get_mut(&remote_id) {
                entry.connections = entry.connections.saturating_sub(1);
                if entry.connections == 0 {
                    peers.remove(&remote_id);
                }
            }
        }
        self.elect();
    }

    /// Follow the smallest connected peer, or lead when none is smaller.
    /// Leadership changes keep the timeline continuous: a new leader adopts
    /// the shared "now" instead of restarting at frame zero.
    fn elect(&self) {
        let mut leader = self.leader.lock().expect("leader lock");
        let next = {
            let peers = self.peers.lock().expect("peers lock");
            peers
                .iter()
                .next()
                .filter(|(id, _)| **id < self.config.peer_id)
                .map(|(id, entry)| Leader {
                    peer_id: *id,
                    epoch_micros: entry.epoch_micros,
                    ping_addr: entry.ping_addr,
                })
        };
        let changed = match (*leader, next) {
            (Some(old), Some(new)) => old.peer_id != new.peer_id,
            (None, None) => false,
            _ => true,
        };
        if !changed {
            return;
        }
        // Offsets measured against the old leader say nothing about the new
        // clock we are about to ping.
        self.estimator.lock().expect("estimator lock").clear();
        if let Some(new) = next {
            debug!(leader = new.peer_id.0, ping = %new.ping_addr, "following new leader");
        } else {
            // We are the smallest peer left: keep the shared timeline
            // running from where it is and lead it ourselves.
            let now = self.config.time.now_micros();
            self.clock.send_if_modified(|clock| {
                let adopted = clock.adopt_lead(now);
                if *clock == adopted {
                    false
                } else {
                    *clock = adopted;
                    true
                }
            });
            debug!("no smaller peer connected; leading the timeline");
        }
        *leader = next;
    }
}

/// Answer inbound pings; when following a leader, also send our own pings
/// and fold replies into the offset estimate.
async fn ping_loop(node: Arc<Node>) {
    let mut ticker = tokio::time::interval(PING_INTERVAL);
    let mut packet = [0_u8; wire::PING_PACKET_BYTES];
    loop {
        tokio::select! {
            _ = ticker.tick() => {
                let target = node.leader.lock().expect("leader lock").map(|leader| leader.ping_addr);
                if let Some(target) = target {
                    let ping = Ping::Request { sent_micros: node.config.time.now_micros() };
                    if let Err(error) = node.udp.send_to(&ping.encode(), target).await {
                        debug!(%error, "clock ping failed");
                    }
                }
            }
            received = node.udp.recv_from(&mut packet) => {
                let Ok((length, from)) = received else { continue };
                let now = node.config.time.now_micros();
                match Ping::decode(&packet[..length]) {
                    Ok(Some(Ping::Request { sent_micros })) => {
                        let reply = Ping::Reply {
                            sent_micros,
                            received_micros: now,
                            replied_micros: node.config.time.now_micros(),
                        };
                        if let Err(error) = node.udp.send_to(&reply.encode(), from).await {
                            debug!(%error, "ping reply failed");
                        }
                    }
                    Ok(Some(Ping::Reply { sent_micros, received_micros, replied_micros })) => {
                        node.fold_ping(PingSample {
                            request_sent: sent_micros,
                            peer_received: received_micros,
                            peer_replied: replied_micros,
                            response_received: now,
                        });
                    }
                    Ok(None) => {} // foreign datagram
                    Err(error) => debug!(%error, "corrupt ping packet"),
                }
            }
        }
    }
}

impl Node {
    /// Fold one completed ping into the estimator and republish the clock.
    fn fold_ping(&self, sample: PingSample) {
        let offset = {
            let mut estimator = self.estimator.lock().expect("estimator lock");
            estimator.record(sample);
            estimator.estimate()
        };
        let (Some(offset), Some(leader)) =
            (offset, *self.leader.lock().expect("leader lock"))
        else {
            return;
        };
        let next = SharedClock::follow(offset, leader.epoch_micros);
        self.clock.send_if_modified(|clock| {
            if *clock == next {
                false
            } else {
                *clock = next;
                true
            }
        });
    }
}
