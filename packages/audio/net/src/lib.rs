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
use tokio::sync::{mpsc, watch};
use tracing::{debug, warn};

use wire::{Clock, Hello, Message, Ping};

/// How often peers gossip score deltas and clock pings.
const GOSSIP_INTERVAL: Duration = Duration::from_millis(250);
const PING_INTERVAL: Duration = Duration::from_millis(500);
const HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(5);
const SCORE_CHUNK_BYTES: usize = 1024 * 1024;
const MAX_SCORE_SYNC_BYTES: usize = 512 * 1024 * 1024;
const OUTBOUND_MESSAGES: usize = 8;

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
    leader: Mutex<Option<Route>>,
    /// A lower-id newcomer first measures the established session, then
    /// takes over without resetting its timeline.
    takeover: Mutex<Option<PeerId>>,
    /// Followers advertise only after measuring their current proxy.
    clock_ready: Mutex<bool>,
    /// Every peer with a live connection, keyed by id; the election in
    /// [`Node::elect`] follows the smallest one, and a peer leaving
    /// (connection count hitting zero) triggers a re-election.
    peers: Mutex<BTreeMap<PeerId, PeerEntry>>,
    udp: UdpSocket,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Route {
    leader_id: PeerId,
    proxy_id: PeerId,
    epoch_micros: i64,
    ping_addr: SocketAddr,
}

#[derive(Debug, Clone, Copy)]
struct PeerEntry {
    ping_addr: SocketAddr,
    epoch_micros: i64,
    leader_id: PeerId,
    ready: bool,
    /// Live connections carrying this peer (we may both dial and accept).
    connections: usize,
}

#[derive(Debug)]
struct Handshake {
    remote: Hello,
    clock: Clock,
    takeover: bool,
}

struct ClockProposal {
    clock: Clock,
    takeover: bool,
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
        takeover: Mutex::new(None),
        clock_ready: Mutex::new(true),
        peers: Mutex::new(BTreeMap::new()),
        udp,
    });

    let mut tasks = Vec::new();
    tasks.push(tokio::spawn(accept_loop(Arc::clone(&node), listener)));
    for peer in node.config.peers.clone() {
        tasks.push(tokio::spawn(dial_loop(Arc::clone(&node), peer)));
    }
    tasks.push(tokio::spawn(ping_loop(Arc::clone(&node))));

    Ok(NodeHandle { tcp_addr, udp_addr, clock: clock_rx, tasks })
}

async fn accept_loop(node: Arc<Node>, listener: TcpListener) {
    let mut connections = tokio::task::JoinSet::new();
    loop {
        tokio::select! {
            accepted = listener.accept() => match accepted {
                Ok((stream, addr)) => {
                    debug!(%addr, "peer connected");
                    connections.spawn(serve_connection(Arc::clone(&node), stream));
                }
                Err(error) => {
                    warn!(%error, "accept failed");
                    tokio::time::sleep(GOSSIP_INTERVAL).await;
                }
            },
            Some(result) = connections.join_next(), if !connections.is_empty() => {
                if let Err(error) = result {
                    warn!(%error, "peer connection task failed");
                }
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

/// Exchange a bounded handshake, then run independent reader and writer
/// paths so a slow frame or simultaneous blob response cannot deadlock TCP.
async fn drive_connection(node: Arc<Node>, mut stream: TcpStream, peer: SocketAddr) -> Result<()> {
    let remote = exchange_handshake(&node, &mut stream, HANDSHAKE_TIMEOUT).await?;

    let Handshake { remote, clock, takeover } = remote;
    let remote_id = PeerId(remote.peer_id);
    node.peer_connected(&remote, peer, clock, takeover);
    let result = connection_loop(&node, stream, remote_id).await;
    node.peer_disconnected(remote_id);
    result
}

async fn exchange_handshake(
    node: &Node,
    stream: &mut TcpStream,
    timeout: Duration,
) -> Result<Handshake> {
    tokio::time::timeout(timeout, async {
        node.hello().write_to(stream).await?;
        let first = Message::read_from(stream)
            .await?
            .context("peer closed before Hello")?;
        let Message::Hello(remote) = first else {
            anyhow::bail!("peer spoke before Hello");
        };
        anyhow::ensure!(
            remote.sample_rate == node.config.sample_rate,
            "peer sample rate {} differs from local {}",
            remote.sample_rate,
            node.config.sample_rate
        );

        let proposal = node.handshake_clock(&remote);
        Message::Clock(proposal.clock).write_to(stream).await?;
        let message = Message::read_from(stream)
            .await?
            .context("peer closed before clock handshake")?;
        let Message::Clock(remote_clock) = message else {
            anyhow::bail!("peer sent data before clock handshake");
        };
        Ok::<_, anyhow::Error>(Handshake {
            remote,
            clock: remote_clock,
            takeover: proposal.takeover && remote_clock.ready,
        })
    })
    .await
    .context("peer Hello timed out")?
}

async fn connection_loop(node: &Arc<Node>, stream: TcpStream, remote_id: PeerId) -> Result<()> {
    let (reader, writer) = stream.into_split();
    let (outbound, messages) = mpsc::channel(OUTBOUND_MESSAGES);
    let requested = Arc::new(Mutex::new(None));
    let read = read_loop(node, reader, remote_id, outbound.clone(), Arc::clone(&requested));
    let write = write_loop(writer, messages);
    let gossip = gossip_loop(node, outbound, requested);
    tokio::pin!(read, write, gossip);
    tokio::select! {
        result = &mut read => result,
        result = &mut write => result,
        result = &mut gossip => result,
    }
}

async fn write_loop(
    mut writer: tokio::net::tcp::OwnedWriteHalf,
    mut messages: mpsc::Receiver<Message>,
) -> Result<()> {
    while let Some(message) = messages.recv().await {
        message.write_to(&mut writer).await?;
    }
    Ok(())
}

async fn read_loop(
    node: &Node,
    mut reader: tokio::net::tcp::OwnedReadHalf,
    remote_id: PeerId,
    outbound: mpsc::Sender<Message>,
    requested: Arc<Mutex<Option<audio_blob::BlobHash>>>,
) -> Result<()> {
    let mut score = Vec::new();
    while let Some(message) = Message::read_from(&mut reader).await? {
        match message {
            Message::ScoreChunk { end, bytes } => {
                anyhow::ensure!(
                    score.len().saturating_add(bytes.len()) <= MAX_SCORE_SYNC_BYTES,
                    "score sync exceeds {MAX_SCORE_SYNC_BYTES} bytes"
                );
                score.extend_from_slice(&bytes);
                if end {
                    let imported = node.score.lock().expect("score lock").import(&score);
                    if let Err(error) = imported {
                        warn!(%error, "dropping malformed score update");
                    }
                    score.clear();
                }
            }
            message => node.apply(message, remote_id, &outbound, &requested).await?,
        }
    }
    Ok(())
}

async fn gossip_loop(
    node: &Node,
    outbound: mpsc::Sender<Message>,
    requested: Arc<Mutex<Option<audio_blob::BlobHash>>>,
) -> Result<()> {
    let mut sent = None;
    let mut advertised = None;
    let mut ticker = tokio::time::interval(GOSSIP_INTERVAL);
    loop {
        ticker.tick().await;
        let update = {
            let score = node.score.lock().expect("score lock");
            let current = score.version();
            let bytes = match sent.as_ref() {
                None => Some(score.export_snapshot()?),
                Some(version) if version != &current => Some(score.export_updates(version)?),
                Some(_) => None,
            };
            drop(score);
            sent = Some(current);
            bytes
        };
        if let Some(update) = update {
            send_score(&outbound, &update).await?;
        }

        let clock = node.advertised_clock();
        if advertised != Some(clock) {
            outbound.send(Message::Clock(clock)).await?;
            advertised = Some(clock);
        }

        let missing = node
            .score
            .lock()
            .expect("score lock")
            .instrument()
            .ok()
            .flatten()
            .map(|instrument| instrument.hash)
            .filter(|hash| !node.store.contains(hash));
        let should_request = {
            let mut pending = requested.lock().expect("requested blob lock");
            let changed = if *pending == missing {
                false
            } else {
                *pending = missing;
                missing.is_some()
            };
            drop(pending);
            changed
        };
        if should_request {
            outbound.send(Message::BlobRequest(missing.expect("missing hash"))).await?;
        }
    }
}

async fn send_score(outbound: &mpsc::Sender<Message>, update: &[u8]) -> Result<()> {
    if update.is_empty() {
        outbound.send(Message::ScoreChunk { end: true, bytes: Vec::new() }).await?;
        return Ok(());
    }
    let chunks = update.chunks(SCORE_CHUNK_BYTES);
    let count = chunks.len();
    for (index, bytes) in chunks.enumerate() {
        outbound
            .send(Message::ScoreChunk { end: index + 1 == count, bytes: bytes.to_vec() })
            .await?;
    }
    Ok(())
}

impl Node {
    fn hello(&self) -> Message {
        Message::Hello(Hello {
            peer_id: self.config.peer_id.0,
            udp_port: self.udp.local_addr().expect("bound UDP socket").port(),
            sample_rate: self.config.sample_rate,
            clock: self.advertised_clock(),
        })
    }

    /// A new lower-id peer first follows the established clock. Its
    /// not-ready advertisement prevents incumbents from switching early.
    fn handshake_clock(&self, remote: &Hello) -> ClockProposal {
        let local = self.advertised_clock();
        let takeover = local.ready
            && remote.clock.ready
            && local.leader_id == self.config.peer_id.0
            && self.config.peer_id.0 < remote.clock.leader_id;
        if takeover {
            ClockProposal {
                clock: Clock {
                    leader_id: remote.clock.leader_id,
                    epoch_micros: remote.clock.epoch_micros,
                    ready: false,
                },
                takeover: true,
            }
        } else {
            ClockProposal { clock: local, takeover: false }
        }
    }

    fn advertised_clock(&self) -> Clock {
        let leader_id = self
            .leader
            .lock()
            .expect("leader lock")
            .map_or(self.config.peer_id, |leader| leader.leader_id);
        Clock {
            leader_id: leader_id.0,
            epoch_micros: self.clock.borrow().local_epoch_micros(),
            ready: *self.clock_ready.lock().expect("clock ready lock"),
        }
    }

    async fn apply(
        &self,
        message: Message,
        remote_id: PeerId,
        outbound: &mpsc::Sender<Message>,
        requested: &Mutex<Option<audio_blob::BlobHash>>,
    ) -> Result<()> {
        match message {
            Message::Hello(_) | Message::ScoreChunk { .. } => {
                anyhow::bail!("peer sent a handshake or score fragment out of sequence");
            }
            Message::Clock(clock) => self.peer_clock(remote_id, clock),
            Message::BlobRequest(hash) => {
                if let Ok(Some(bytes)) = self.store.get(&hash) {
                    outbound.send(Message::Blob(hash, bytes)).await?;
                }
            }
            Message::Blob(hash, bytes) => {
                let expected = requested.lock().expect("requested blob lock").take() == Some(hash);
                let referenced = self
                    .score
                    .lock()
                    .expect("score lock")
                    .instrument()
                    .ok()
                    .flatten()
                    .is_some_and(|instrument| instrument.hash == hash);
                if !expected || !referenced {
                    warn!(%hash, "ignoring unsolicited blob");
                } else {
                    let stored = audio_blob::BlobHash::of(&bytes);
                    if stored == hash {
                        self.store.put(&bytes)?;
                    } else {
                        warn!(claimed = %hash, actual = %stored, "peer sent mismatched blob");
                    }
                }
            }
        }
        Ok(())
    }

    fn peer_connected(
        &self,
        remote: &Hello,
        tcp_addr: SocketAddr,
        clock: Clock,
        takeover: bool,
    ) {
        let remote_id = PeerId(remote.peer_id);
        let mut ping_addr = tcp_addr;
        ping_addr.set_port(remote.udp_port);
        let mut peers = self.peers.lock().expect("peers lock");
        let entry = peers.entry(remote_id).or_insert(PeerEntry {
            ping_addr,
            epoch_micros: clock.epoch_micros,
            leader_id: PeerId(clock.leader_id),
            ready: clock.ready,
            connections: 0,
        });
        entry.connections += 1;
        entry.ping_addr = ping_addr;
        entry.epoch_micros = clock.epoch_micros;
        entry.leader_id = PeerId(clock.leader_id);
        entry.ready = clock.ready;
        drop(peers);
        if takeover {
            *self.takeover.lock().expect("takeover lock") = Some(remote_id);
        }
        self.elect();
    }

    fn peer_clock(&self, remote_id: PeerId, clock: Clock) {
        if let Some(entry) = self.peers.lock().expect("peers lock").get_mut(&remote_id) {
            entry.epoch_micros = clock.epoch_micros;
            entry.leader_id = PeerId(clock.leader_id);
            entry.ready = clock.ready;
        }
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
        let mut takeover = self.takeover.lock().expect("takeover lock");
        if *takeover == Some(remote_id) {
            *takeover = None;
        }
        drop(takeover);
        self.elect();
    }

    /// Follow the route advertising the smallest effective leader. A
    /// follower can therefore proxy a leader outside our direct topology.
    fn elect(&self) {
        let mut leader = self.leader.lock().expect("leader lock");
        let next = {
            let peers = self.peers.lock().expect("peers lock");
            let takeover = *self.takeover.lock().expect("takeover lock");
            peers
                .iter()
                .filter(|(id, entry)| {
                    entry.ready
                        && (Some(**id) == takeover || entry.leader_id < self.config.peer_id)
                })
                .min_by_key(|(id, entry)| (entry.leader_id, **id))
                .map(|(id, entry)| Route {
                    leader_id: entry.leader_id,
                    proxy_id: *id,
                    epoch_micros: entry.epoch_micros,
                    ping_addr: entry.ping_addr,
                })
        };
        if *leader == next {
            return;
        }
        self.estimator.lock().expect("estimator lock").clear();
        if let Some(new) = next {
            *self.clock_ready.lock().expect("clock ready lock") = false;
            debug!(leader = new.leader_id.0, proxy = new.proxy_id.0, ping = %new.ping_addr, "following clock route");
        } else {
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
            *self.clock_ready.lock().expect("clock ready lock") = true;
            debug!("leading the timeline");
        }
        *leader = next;
    }
}


/// Answer inbound pings; when following a leader, also send our own pings
/// and fold replies into the offset estimate.
async fn ping_loop(node: Arc<Node>) {
    let mut ticker = tokio::time::interval(PING_INTERVAL);
    let mut packet = [0_u8; wire::PING_PACKET_BYTES];
    let mut outstanding = None;
    loop {
        tokio::select! {
            _ = ticker.tick() => {
                let target = node.leader.lock().expect("leader lock").map(|leader| leader.ping_addr);
                if let Some(target) = target {
                    let sent_micros = node.config.time.now_micros();
                    let ping = Ping::Request { sent_micros };
                    if let Err(error) = node.udp.send_to(&ping.encode(), target).await {
                        debug!(%error, "clock ping failed");
                    } else {
                        outstanding = Some((target, sent_micros));
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
                        let target = node
                            .leader
                            .lock()
                            .expect("leader lock")
                            .map(|leader| leader.ping_addr);
                        if reply_matches(outstanding, target, from, sent_micros) {
                            outstanding = None;
                            node.fold_ping(PingSample {
                                request_sent: sent_micros,
                                peer_received: received_micros,
                                peer_replied: replied_micros,
                                response_received: now,
                            });
                        } else {
                            debug!(%from, "ignoring unsolicited clock reply");
                        }
                    }
                    Ok(None) => {} // foreign datagram
                    Err(error) => debug!(%error, "corrupt ping packet"),
                }
            }
        }
    }
}

fn reply_matches(
    outstanding: Option<(SocketAddr, u64)>,
    target: Option<SocketAddr>,
    from: SocketAddr,
    sent_micros: u64,
) -> bool {
    outstanding == Some((from, sent_micros)) && target == Some(from)
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
        *self.clock_ready.lock().expect("clock ready lock") = true;

        let mut takeover = self.takeover.lock().expect("takeover lock");
        if *takeover == Some(leader.proxy_id) {
            *takeover = None;
            drop(takeover);
            self.elect();
        }
    }
}

#[cfg(test)]
mod tests {
    use std::net::{Ipv6Addr, SocketAddrV6};
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicU64, Ordering};

    use tokio::io::AsyncWriteExt as _;

    use super::*;

    #[derive(Default)]
    struct TestTime(AtomicU64);

    impl MonotonicTime for TestTime {
        fn now_micros(&self) -> u64 {
            self.0.fetch_add(1_000, Ordering::Relaxed)
        }
    }

    struct TestDir(PathBuf);

    impl TestDir {
        fn new(label: &str) -> Self {
            static NEXT: AtomicU64 = AtomicU64::new(0);
            let path = std::env::temp_dir().join(format!(
                "audio-net-{label}-{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            ));
            Self(path)
        }
    }

    impl Drop for TestDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    struct TestNode {
        node: Arc<Node>,
        _dir: TestDir,
    }

    async fn test_node(id: u64, label: &str) -> TestNode {
        let dir = TestDir::new(label);
        let udp = UdpSocket::bind("127.0.0.1:0").await.expect("bind UDP");
        let now = 1_000_000;
        let (clock, _) = watch::channel(SharedClock::lead(now));
        let node = Node {
            config: Config {
                peer_id: PeerId(id),
                tcp_bind: "127.0.0.1:0".parse().expect("TCP address"),
                udp_bind: "127.0.0.1:0".parse().expect("UDP address"),
                peers: Vec::new(),
                sample_rate: 48_000,
                time: Arc::new(TestTime::default()),
            },
            score: Arc::new(Mutex::new(Score::new())),
            store: Arc::new(BlobStore::open(&dir.0).expect("blob store")),
            clock,
            estimator: Mutex::new(OffsetEstimator::new(16)),
            leader: Mutex::new(None),
            takeover: Mutex::new(None),
            clock_ready: Mutex::new(true),
            peers: Mutex::new(BTreeMap::new()),
            udp,
        };
        TestNode { node: Arc::new(node), _dir: dir }
    }

    fn hello(id: u64, clock: Clock) -> Hello {
        Hello { peer_id: id, udp_port: 9000, sample_rate: 48_000, clock }
    }

    #[tokio::test]
    async fn lower_id_bootstraps_before_advertising_takeover() {
        let test = test_node(1, "takeover").await;
        let node = test.node;
        let remote = hello(
            3,
            Clock { leader_id: 3, epoch_micros: 500_000, ready: true },
        );
        let proposal = node.handshake_clock(&remote);
        assert!(proposal.takeover);
        assert_eq!(proposal.clock.leader_id, 3);
        assert!(!proposal.clock.ready);

        node.peer_connected(
            &remote,
            "127.0.0.1:8000".parse().expect("peer address"),
            remote.clock,
            true,
        );
        node.fold_ping(PingSample {
            request_sent: 0,
            peer_received: 100,
            peer_replied: 100,
            response_received: 0,
        });
        assert_eq!(*node.takeover.lock().expect("takeover lock"), None);
        assert_eq!(*node.leader.lock().expect("leader lock"), None);
        let advertised = node.advertised_clock();
        assert_eq!(advertised.leader_id, 1);
        assert!(advertised.ready);
    }

    #[tokio::test]
    async fn election_uses_hidden_leader_and_refreshes_epoch() {
        let test = test_node(2, "hidden-leader").await;
        let node = test.node;
        let remote = hello(
            3,
            Clock { leader_id: 1, epoch_micros: 400_000, ready: true },
        );
        node.peer_connected(
            &remote,
            "127.0.0.1:8000".parse().expect("peer address"),
            remote.clock,
            false,
        );
        let leader = node.leader.lock().expect("leader lock").expect("leader");
        assert_eq!(leader.leader_id, PeerId(1));
        assert_eq!(leader.proxy_id, PeerId(3));

        node.estimator.lock().expect("estimator lock").record(PingSample {
            request_sent: 0,
            peer_received: 10,
            peer_replied: 10,
            response_received: 20,
        });
        node.peer_clock(
            PeerId(3),
            Clock { leader_id: 1, epoch_micros: 450_000, ready: true },
        );
        assert!(node.estimator.lock().expect("estimator lock").is_empty());
        assert_eq!(
            node.leader.lock().expect("leader lock").expect("leader").epoch_micros,
            450_000
        );
        node.peer_disconnected(PeerId(3));
        assert_eq!(*node.leader.lock().expect("leader lock"), None);
        assert!(node.advertised_clock().ready);
    }

    #[tokio::test]
    async fn peer_clock_address_preserves_ipv6_scope() {
        let test = test_node(10, "ipv6").await;
        let node = test.node;
        let remote = hello(
            1,
            Clock { leader_id: 1, epoch_micros: 0, ready: true },
        );
        let tcp = SocketAddr::V6(SocketAddrV6::new(
            Ipv6Addr::LOCALHOST,
            8000,
            7,
            42,
        ));
        node.peer_connected(&remote, tcp, remote.clock, false);
        let ping = node.peers.lock().expect("peers lock")[&PeerId(1)].ping_addr;
        let SocketAddr::V6(ping) = ping else { panic!("IPv6 address") };
        assert_eq!(ping.port(), 9000);
        assert_eq!(ping.flowinfo(), 7);
        assert_eq!(ping.scope_id(), 42);
    }

    #[test]
    fn clock_replies_must_match_target_and_request() {
        let leader: SocketAddr = "127.0.0.1:9000".parse().expect("leader address");
        let other: SocketAddr = "127.0.0.1:9001".parse().expect("other address");
        assert!(reply_matches(Some((leader, 7)), Some(leader), leader, 7));
        assert!(!reply_matches(Some((leader, 7)), Some(leader), other, 7));
        assert!(!reply_matches(Some((leader, 7)), Some(leader), leader, 8));
    }

    #[tokio::test]
    async fn large_score_sync_is_chunked_below_frame_cap() -> Result<()> {
        let update = vec![42; wire::MAX_FRAME_BYTES as usize + 17];
        let (tx, mut rx) = mpsc::channel(OUTBOUND_MESSAGES);
        let receive = async {
            let mut rebuilt = Vec::new();
            while let Some(message) = rx.recv().await {
                let Message::ScoreChunk { end, bytes } = message else {
                    panic!("score chunk");
                };
                assert!(message_size(&bytes) <= wire::MAX_FRAME_BYTES as usize);
                rebuilt.extend_from_slice(&bytes);
                if end {
                    break;
                }
            }
            rebuilt
        };
        let (sent, rebuilt) = tokio::join!(send_score(&tx, &update), receive);
        sent?;
        assert_eq!(rebuilt, update);
        Ok(())
    }

    const fn message_size(bytes: &[u8]) -> usize {
        1 + 1 + bytes.len()
    }

    #[tokio::test]
    async fn fragmented_frame_survives_gossip_ticks() -> Result<()> {
        let test = test_node(10, "fragmented").await;
        let node = test.node;
        let source = Score::new();
        source.set_control(4, 0.75, 0)?;
        let frame = Message::ScoreChunk { end: true, bytes: source.export_snapshot()? }.encode()?;
        let listener = TcpListener::bind("127.0.0.1:0").await?;
        let client = TcpStream::connect(listener.local_addr()?).await?;
        let (server, _) = listener.accept().await?;
        let connection = connection_loop(&node, server, PeerId(1));
        let sender = async move {
            let mut client = client;
            client.write_all(&frame[..6]).await?;
            tokio::time::sleep(GOSSIP_INTERVAL + Duration::from_millis(50)).await;
            client.write_all(&frame[6..]).await?;
            tokio::time::sleep(Duration::from_millis(20)).await;
            Ok::<_, anyhow::Error>(())
        };
        let (_, sent_result) = tokio::join!(connection, sender);
        sent_result?;
        let value = node.score.lock().expect("score lock").controls_at(0)[0].value;
        assert!((value - 0.75).abs() < f32::EPSILON);
        Ok(())
    }

    #[tokio::test]
    async fn dropping_node_releases_inbound_connection_socket() -> Result<()> {
        let a_dir = TestDir::new("drop-a");
        let b_dir = TestDir::new("drop-b");
        let a = spawn(
            Config {
                peer_id: PeerId(1),
                tcp_bind: "127.0.0.1:0".parse()?,
                udp_bind: "127.0.0.1:0".parse()?,
                peers: Vec::new(),
                sample_rate: 48_000,
                time: Arc::new(TestTime::default()),
            },
            Arc::new(Mutex::new(Score::new())),
            Arc::new(BlobStore::open(&a_dir.0)?),
        )
        .await?;
        let udp_addr = a.udp_addr;
        let b = spawn(
            Config {
                peer_id: PeerId(2),
                tcp_bind: "127.0.0.1:0".parse()?,
                udp_bind: "127.0.0.1:0".parse()?,
                peers: vec![a.tcp_addr],
                sample_rate: 48_000,
                time: Arc::new(TestTime::default()),
            },
            Arc::new(Mutex::new(Score::new())),
            Arc::new(BlobStore::open(&b_dir.0)?),
        )
        .await?;
        tokio::time::sleep(Duration::from_millis(50)).await;
        drop(a);
        tokio::task::yield_now().await;
        let rebound = UdpSocket::bind(udp_addr).await?;
        drop(rebound);
        drop(b);
        Ok(())
    }

    #[tokio::test]
    async fn handshake_rejects_sample_rate_mismatch_and_stalls() -> Result<()> {
        let test = test_node(10, "handshake").await;
        let node = test.node;
        let listener = TcpListener::bind("127.0.0.1:0").await?;
        let mut client = TcpStream::connect(listener.local_addr()?).await?;
        let (mut server, _) = listener.accept().await?;
        let peer = tokio::spawn(async move {
            Message::read_from(&mut server).await?;
            Message::Hello(Hello {
                peer_id: 1,
                udp_port: 9000,
                sample_rate: 44_100,
                clock: Clock { leader_id: 1, epoch_micros: 0, ready: true },
            })
            .write_to(&mut server)
            .await
        });
        let error = exchange_handshake(&node, &mut client, HANDSHAKE_TIMEOUT)
            .await
            .expect_err("sample-rate mismatch");
        assert!(error.to_string().contains("sample rate"));
        peer.await??;

        let listener = TcpListener::bind("127.0.0.1:0").await?;
        let mut client = TcpStream::connect(listener.local_addr()?).await?;
        let stalled = tokio::spawn(async move {
            let (_server, _) = listener.accept().await.expect("accept stalled peer");
            tokio::time::sleep(Duration::from_millis(100)).await;
        });
        let error = exchange_handshake(&node, &mut client, Duration::from_millis(10))
            .await
            .expect_err("handshake timeout");
        assert!(error.to_string().contains("timed out"));
        stalled.await?;
        Ok(())
    }

    #[tokio::test]
    async fn unsolicited_blobs_are_not_persisted() -> Result<()> {
        let test = test_node(10, "blob-request").await;
        let node = test.node;
        let bytes = b"instrument".to_vec();
        let hash = audio_blob::BlobHash::of(&bytes);
        node.score.lock().expect("score lock").set_instrument(&hash, 0)?;
        let (outbound, _messages) = mpsc::channel(1);
        let requested = Mutex::new(None);
        node.apply(Message::Blob(hash, bytes.clone()), PeerId(1), &outbound, &requested)
            .await?;
        assert!(!node.store.contains(&hash));

        *requested.lock().expect("requested blob lock") = Some(hash);
        node.apply(Message::Blob(hash, bytes), PeerId(1), &outbound, &requested).await?;
        assert!(node.store.contains(&hash));
        Ok(())
    }
}
