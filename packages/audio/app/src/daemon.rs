//! The `shared-audio daemon`: joins the LAN session, plays the shared
//! timeline, and serves the control socket. Designed to run under
//! launchd/systemd (see this repo's `homeModules.portable-services`).

use std::collections::BTreeSet;
use std::net::SocketAddr;
use std::os::unix::fs::PermissionsExt as _;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use anyhow::{Context as _, Result, ensure};
use audio_blob::{BlobHash, BlobStore};
use audio_clock::{MonotonicTime, PeerId, ProcessTime, SAMPLE_RATE, SharedClock};
use audio_engine::{Player, PlayerSpawn, Renderer, Volume};
use audio_instrument::{CONTROL_COUNT, Instrument};
use audio_net::NodeHandle;
use audio_score::Score;
use base64::Engine as _;
use tokio::io::{AsyncBufReadExt as _, AsyncWriteExt as _, BufReader};
use tokio::net::UnixListener;
use tracing::{info, warn};

use crate::control::{self, Request, Response, Status};

/// The instrument every fresh session starts with, so `shared-audio daemon`
/// makes sound out of the box before anyone publishes a module.
pub const DEFAULT_INSTRUMENT_WAT: &str = include_str!("default_instrument.wat");

/// How often the score snapshot is persisted when it changed.
const PERSIST_INTERVAL: Duration = Duration::from_secs(5);

/// Largest blob that fits the wire's tag, SHA-256 hash, and frame cap.
#[expect(clippy::cast_lossless, reason = "Unix usize is at least 32 bits")]
const MAX_INSTRUMENT_BYTES: usize = audio_net::wire::MAX_FRAME_BYTES as usize - 1 - 32;

#[derive(Debug, clap::Args)]
pub struct Opts {
    /// Static peer addresses to gossip with (repeatable). LAN discovery is
    /// deliberately injected: point peers at each other explicitly, from
    /// config, or from a future mDNS feeder.
    #[arg(long = "peer")]
    pub peers: Vec<String>,
    /// TCP bind address for score/blob gossip.
    #[arg(long, default_value = "0.0.0.0:7648")]
    pub tcp_bind: std::net::SocketAddr,
    /// UDP bind address for clock pings.
    #[arg(long, default_value = "0.0.0.0:7649")]
    pub udp_bind: std::net::SocketAddr,
    /// Join and gossip but keep the speaker closed (CI, headless relays).
    #[arg(long)]
    pub no_audio: bool,
    /// Override the state directory (score snapshot, blobs, socket).
    #[arg(long)]
    pub state_dir: Option<PathBuf>,
}

pub fn run(opts: Opts) -> Result<()> {
    tokio::runtime::Runtime::new()?.block_on(run_async(opts))
}

/// Everything a control request can touch.
pub struct State {
    pub score: Arc<Mutex<Score>>,
    pub store: Arc<BlobStore>,
    pub volume: Volume,
    pub node: Arc<NodeHandle>,
    pub time: Arc<dyn MonotonicTime>,
    pub peer_id: PeerId,
    pub sample_rate: u32,
    /// The self-led clock held while this daemon is still joining a session.
    pub pending_session_clock: Option<SharedClock>,
}

async fn run_async(opts: Opts) -> Result<()> {
    let state_dir = opts.state_dir.unwrap_or_else(control::state_dir);
    prepare_state_dir(&state_dir)?;
    let blobs = Arc::new(BlobStore::open(state_dir.join("blobs"))?);

    let score_path = state_dir.join("score.loro");
    let score = Score::new();
    if let Ok(snapshot) = std::fs::read(&score_path) {
        score.import(&snapshot).context("import persisted score")?;
        info!(path = %score_path.display(), "restored score snapshot");
    }
    let joining_session = !opts.peers.is_empty();
    if let Some(hash) = seed_default_instrument(&score, &blobs, joining_session)? {
        info!(%hash, "seeded default instrument");
    }
    let sample_rate = validated_sample_rate(&score)?;
    score.set_sample_rate(sample_rate)?;
    let score = Arc::new(Mutex::new(score));

    let peer_id = load_or_create_peer_id(&state_dir)?;
    let time: Arc<dyn MonotonicTime> = Arc::new(ProcessTime::default());
    let peers = resolve_peers(&opts.peers).await?;
    let node = Arc::new(
        audio_net::spawn(
            audio_net::Config {
                peer_id,
                tcp_bind: opts.tcp_bind,
                udp_bind: opts.udp_bind,
                peers,
                sample_rate,
                time: Arc::clone(&time),
            },
            Arc::clone(&score),
            Arc::clone(&blobs),
        )
        .await?,
    );
    info!(peer = peer_id.0, tcp = %node.tcp_addr, udp = %node.udp_addr, "node up");

    let volume = Volume::default();
    let _audio = if opts.no_audio {
        None
    } else {
        Some(start_audio(&score, &blobs, &node, &time, sample_rate, &volume)?)
    };

    let pending_session_clock = joining_session.then_some(node.clock());
    let state = Arc::new(State {
        score: Arc::clone(&score),
        store: blobs,
        volume,
        node,
        time,
        peer_id,
        sample_rate,
        pending_session_clock,
    });

    let persist_task = tokio::spawn(persist_loop(Arc::clone(&score), score_path.clone()));

    let socket_path = control::socket_path_in(&state_dir);
    let listener = bind_control_listener(&socket_path).await?;
    info!(socket = %socket_path.display(), "control socket ready");

    loop {
        tokio::select! {
            accepted = listener.accept() => {
                let (stream, _) = accepted?;
                tokio::spawn(serve_client(stream, Arc::clone(&state)));
            }
            () = shutdown_signal() => break,
        }
    }
    persist_task.abort();
    let _ = persist_task.await;
    let persist_result = persist_score(&score, &score_path);
    let _ = std::fs::remove_file(&socket_path);
    persist_result?;
    info!("daemon stopped");
    Ok(())
}

fn prepare_state_dir(path: &Path) -> Result<()> {
    std::fs::create_dir_all(path).with_context(|| format!("create state dir {}", path.display()))?;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o700))
        .with_context(|| format!("restrict state dir {}", path.display()))?;
    Ok(())
}

fn validated_sample_rate(score: &Score) -> Result<u32> {
    let sample_rate = score.sample_rate().unwrap_or(SAMPLE_RATE);
    ensure!(sample_rate > 0, "persisted sample rate must be greater than zero");
    Ok(sample_rate)
}

fn seed_default_instrument(
    score: &Score,
    blobs: &BlobStore,
    joining_session: bool,
) -> Result<Option<BlobHash>> {
    if joining_session || score.instrument()?.is_some() {
        return Ok(None);
    }
    let hash = blobs.put(DEFAULT_INSTRUMENT_WAT.as_bytes())?;
    score.set_instrument(&hash, 0)?;
    Ok(Some(hash))
}

async fn resolve_peers(peers: &[String]) -> Result<Vec<SocketAddr>> {
    let mut resolved = BTreeSet::new();
    for peer in peers {
        let addresses = tokio::net::lookup_host(peer)
            .await
            .with_context(|| format!("resolve peer {peer}"))?;
        resolved.extend(addresses);
    }
    Ok(resolved.into_iter().collect())
}

fn load_or_create_peer_id(state_dir: &Path) -> Result<PeerId> {
    let path = state_dir.join("peer-id");
    if path.exists() {
        return read_peer_id(&path);
    }

    let candidate = PeerId::random();
    let tmp = state_dir.join(format!("peer-id.{}.tmp", candidate.0));
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&tmp)
        .with_context(|| format!("create peer identity {}", tmp.display()))?;
    serde_json::to_writer(&mut file, &candidate.0)?;
    file.sync_all()?;
    let linked = std::fs::hard_link(&tmp, &path);
    let _ = std::fs::remove_file(&tmp);
    match linked {
        Ok(()) => {
            std::fs::File::open(state_dir)?.sync_all()?;
            Ok(candidate)
        }
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => read_peer_id(&path),
        Err(error) => Err(error).with_context(|| format!("install peer identity {}", path.display())),
    }
}

async fn bind_control_listener(path: &Path) -> Result<UnixListener> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    // Only remove a stale socket file. A live daemon answers a connect, and
    // stealing its pathname would silently reroute every client.
    match tokio::net::UnixStream::connect(path).await {
        Ok(_) => anyhow::bail!("another daemon is already serving {}", path.display()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(_) => {
            let _ = std::fs::remove_file(path);
        }
    }
    let listener = UnixListener::bind(path)
        .with_context(|| format!("bind control socket {}", path.display()))?;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))
        .with_context(|| format!("restrict control socket {}", path.display()))?;
    Ok(listener)
}

fn read_peer_id(path: &Path) -> Result<PeerId> {
    let text = std::fs::read_to_string(path)
        .with_context(|| format!("read peer identity {}", path.display()))?;
    let id = serde_json::from_str(&text)
        .with_context(|| format!("parse peer identity {}", path.display()))?;
    Ok(PeerId(id))
}

/// Open the default output device and start the schedule-ahead player.
fn start_audio(
    score: &Arc<Mutex<Score>>,
    blobs: &Arc<BlobStore>,
    node: &Arc<NodeHandle>,
    time: &Arc<dyn MonotonicTime>,
    sample_rate: u32,
    volume: &Volume,
) -> Result<AudioStack> {
    let renderer = Renderer::new(Arc::clone(score), Arc::clone(blobs));
    let clock_node = Arc::clone(node);
    let PlayerSpawn { player, source } = Player::spawn(
        renderer,
        move || clock_node.clock(),
        Arc::clone(time),
        sample_rate,
        volume.clone(),
    );
    let mut device = rodio::DeviceSinkBuilder::open_default_sink()
        .map_err(anyhow::Error::from)
        .context("open default audio output")?;
    device.log_on_drop(false);
    let sink = rodio::Player::connect_new(device.mixer());
    sink.append(source);
    Ok(AudioStack { _player: player, _device: device, _sink: sink })
}

/// Live audio output held by the daemon for its lifetime; dropping it stops
/// the render thread and closes the output device.
struct AudioStack {
    _player: Player,
    _device: rodio::MixerDeviceSink,
    _sink: rodio::Player,
}

async fn shutdown_signal() {
    use tokio::signal::unix::{SignalKind, signal};
    let mut term = signal(SignalKind::terminate()).expect("install SIGTERM handler");
    tokio::select! {
        _ = tokio::signal::ctrl_c() => {}
        _ = term.recv() => {}
    }
}

/// Persist the score snapshot whenever its version moved.
async fn persist_loop(score: Arc<Mutex<Score>>, path: PathBuf) {
    let mut interval = tokio::time::interval(PERSIST_INTERVAL);
    let mut persisted = audio_score::VersionVector::new();
    loop {
        interval.tick().await;
        let version = score.lock().expect("score lock").version();
        if version == persisted {
            continue;
        }
        match persist_score(&score, &path) {
            Ok(()) => persisted = version,
            Err(error) => warn!(%error, path = %path.display(), "score persist failed"),
        }
    }
}

fn persist_score(score: &Mutex<Score>, path: &Path) -> Result<()> {
    let snapshot = score.lock().expect("score lock").export_snapshot()?;
    let tmp = path.with_extension("loro.tmp");
    std::fs::write(&tmp, snapshot)
        .and_then(|()| std::fs::rename(&tmp, path))
        .with_context(|| format!("persist score {}", path.display()))?;
    Ok(())
}

async fn serve_client(stream: tokio::net::UnixStream, state: Arc<State>) {
    let (read, mut write) = stream.into_split();
    let mut lines = BufReader::new(read).lines();
    while let Ok(Some(line)) = lines.next_line().await {
        let response = match serde_json::from_str::<Request>(&line) {
            Ok(request) => handle(&state, request),
            Err(error) => Response::err(format!("bad request: {error}")),
        };
        let mut payload = serde_json::to_string(&response).expect("response serializes");
        payload.push('\n');
        if write.write_all(payload.as_bytes()).await.is_err() {
            break;
        }
    }
}

/// Dispatch one control request. Volume requests never touch the score;
/// a unit test below holds that line.
pub fn handle(state: &State, request: Request) -> Response {
    match try_handle(state, request) {
        Ok(response) => response,
        Err(error) => Response::err(format!("{error:#}")),
    }
}

fn try_handle(state: &State, request: Request) -> Result<Response> {
    match request {
        Request::Status => {
            let clock = state.node.clock();
            let frame_now = clock.frame_at(state.time.now_micros(), state.sample_rate);
            let score = state.score.lock().expect("score lock");
            Ok(Response {
                ok: true,
                error: None,
                status: Some(Status {
                    peer_id: state.peer_id.0,
                    tcp_addr: state.node.tcp_addr.to_string(),
                    udp_addr: state.node.udp_addr.to_string(),
                    sample_rate: state.sample_rate,
                    frame_now,
                    epoch_micros: clock.epoch_micros(),
                    gain: state.volume.gain(),
                    muted: state.volume.muted(),
                    instrument: score.instrument()?.map(|i| i.hash.to_string()),
                    controls: score
                        .controls_at(nonnegative_frame(frame_now))
                        .into_iter()
                        .map(|c| (c.control, c.value))
                        .collect(),
                    events: score.events().len(),
                }),
            })
        }
        Request::Volume { set, step, muted } => {
            if let Some(gain) = set {
                state.volume.set_gain(gain);
            }
            if let Some(delta) = step {
                state.volume.step(delta);
            }
            if let Some(muted) = muted {
                state.volume.set_muted(muted);
            }
            Ok(Response::ok())
        }
        Request::Publish { wasm_base64, at_frame } => {
            let bytes = base64::engine::general_purpose::STANDARD
                .decode(wasm_base64)
                .context("decode wasm_base64")?;
            ensure_instrument_fits(bytes.len())?;
            // Validate before it can reach any peer's score.
            Instrument::load(&bytes).context("instrument rejected")?;
            let at_frame = at_frame.map_or_else(|| one_second_out(state), Ok)?;
            let hash = state.store.put(&bytes)?;
            state
                .score
                .lock()
                .expect("score lock")
                .set_instrument(&hash, at_frame)?;
            info!(%hash, at_frame, "instrument published");
            Ok(Response::ok())
        }
        Request::SetControl { control, value } => {
            ensure_control_index(control)?;
            let at_frame = nonnegative_frame(synchronized_frame_now(state)?);
            state
                .score
                .lock()
                .expect("score lock")
                .set_control(control, value, at_frame)?;
            Ok(Response::ok())
        }
        Request::Schedule { at_frame, control, value } => {
            ensure_control_index(control)?;
            state
                .score
                .lock()
                .expect("score lock")
                .schedule(audio_score::Event { at_frame, control, value })?;
            Ok(Response::ok())
        }
    }
}

fn ensure_instrument_fits(length: usize) -> Result<()> {
    ensure!(
        length <= MAX_INSTRUMENT_BYTES,
        "instrument is {length} bytes; gossip supports at most {MAX_INSTRUMENT_BYTES}"
    );
    Ok(())
}

fn ensure_control_index(control: u16) -> Result<()> {
    ensure!(
        usize::from(control) < CONTROL_COUNT,
        "control {control} is outside 0..{CONTROL_COUNT}"
    );
    Ok(())
}

fn nonnegative_frame(frame: i64) -> u64 {
    frame.max(0).unsigned_abs()
}

/// The shared frame one second from now; the default publish switch point.
fn one_second_out(state: &State) -> Result<u64> {
    let now = synchronized_frame_now(state)?;
    Ok((now + i64::from(state.sample_rate)).max(0).unsigned_abs())
}

fn synchronized_frame_now(state: &State) -> Result<i64> {
    let clock = state.node.clock();
    if state.pending_session_clock == Some(clock) {
        anyhow::bail!("session clock is not synchronized yet; retry when synchronization completes");
    }
    Ok(clock.frame_at(state.time.now_micros(), state.sample_rate))
}

#[cfg(test)]
mod tests {
    use super::*;

    struct TestState {
        state: Arc<State>,
        _dir: tempfile::TempDir,
    }

    async fn test_state() -> Result<TestState> {
        test_state_with_pending_clock(false).await
    }

    async fn test_state_with_pending_clock(pending_clock: bool) -> Result<TestState> {
        let dir = tempfile::tempdir()?;
        let blobs = Arc::new(BlobStore::open(dir.path().join("blobs"))?);
        let score = Arc::new(Mutex::new(Score::new()));
        let time: Arc<dyn MonotonicTime> = Arc::new(ProcessTime::default());
        let peer_id = PeerId(7);
        let node = Arc::new(
            audio_net::spawn(
                audio_net::Config {
                    peer_id,
                    tcp_bind: "127.0.0.1:0".parse()?,
                    udp_bind: "127.0.0.1:0".parse()?,
                    peers: vec![],
                    sample_rate: SAMPLE_RATE,
                    time: Arc::clone(&time),
                },
                Arc::clone(&score),
                Arc::clone(&blobs),
            )
            .await?,
        );
        let pending_session_clock = pending_clock.then_some(node.clock());
        Ok(TestState {
            state: Arc::new(State {
                score,
                store: blobs,
                volume: Volume::default(),
                node,
                time,
                peer_id,
                sample_rate: SAMPLE_RATE,
                pending_session_clock,
            }),
            _dir: dir,
        })
    }

    #[tokio::test]
    async fn volume_requests_never_touch_the_score() -> Result<()> {
        let TestState { state, _dir } = test_state().await?;
        let before = state.score.lock().expect("lock").version();
        let response = handle(
            &state,
            Request::Volume { set: Some(0.4), step: Some(-0.1), muted: Some(true) },
        );
        assert!(response.ok);
        let after = state.score.lock().expect("lock").version();
        assert_eq!(before, after, "volume must stay local");
        assert!((state.volume.gain() - 0.3).abs() < f32::EPSILON);
        assert!(state.volume.muted());
        Ok(())
    }

    #[tokio::test]
    async fn publish_rejects_invalid_modules() -> Result<()> {
        let TestState { state, _dir } = test_state().await?;
        let bogus = base64::engine::general_purpose::STANDARD.encode(b"not wasm");
        let response = handle(
            &state,
            Request::Publish { wasm_base64: bogus, at_frame: Some(0) },
        );
        assert!(!response.ok);
        assert!(state.score.lock().expect("lock").instrument()?.is_none());
        Ok(())
    }

    #[tokio::test]
    async fn control_requests_reject_indexes_outside_the_abi() -> Result<()> {
        let TestState { state, _dir } = test_state().await?;
        let invalid = u16::try_from(CONTROL_COUNT).expect("control count fits u16");

        let set = handle(&state, Request::SetControl { control: invalid, value: 1.0 });
        assert!(!set.ok);
        assert!(state.score.lock().expect("lock").controls_at(0).is_empty());

        let schedule = handle(
            &state,
            Request::Schedule { at_frame: 48_000, control: invalid, value: 1.0 },
        );
        assert!(!schedule.ok);
        assert!(state.score.lock().expect("lock").events().is_empty());
        Ok(())
    }

    #[test]
    fn rejects_instruments_that_cannot_fit_a_gossip_frame() {
        ensure_instrument_fits(MAX_INSTRUMENT_BYTES).expect("largest supported instrument");
        let error = ensure_instrument_fits(MAX_INSTRUMENT_BYTES + 1)
            .expect_err("over-cap instrument must fail");
        assert!(error.to_string().contains("gossip supports at most"));
    }

    #[tokio::test]
    async fn default_publish_waits_for_a_joined_session_clock() -> Result<()> {
        let TestState { state, _dir } = test_state_with_pending_clock(true).await?;
        let module = base64::engine::general_purpose::STANDARD
            .encode(DEFAULT_INSTRUMENT_WAT.as_bytes());
        let response = handle(
            &state,
            Request::Publish { wasm_base64: module, at_frame: None },
        );
        assert!(!response.ok);
        assert!(response.error.as_deref().is_some_and(|error| error.contains("not synchronized")));
        assert!(state.score.lock().expect("lock").instrument()?.is_none());

        let module = base64::engine::general_purpose::STANDARD
            .encode(DEFAULT_INSTRUMENT_WAT.as_bytes());
        let response = handle(
            &state,
            Request::Publish { wasm_base64: module, at_frame: Some(123) },
        );
        assert!(response.ok);
        assert_eq!(state.score.lock().expect("lock").instrument()?.expect("instrument").at_frame, 123);
        Ok(())
    }

    #[tokio::test]
    async fn immediate_control_waits_for_a_joined_session_clock() -> Result<()> {
        let TestState { state, _dir } = test_state_with_pending_clock(true).await?;
        let response = handle(&state, Request::SetControl { control: 0, value: 0.5 });

        assert!(!response.ok);
        assert!(response.error.as_deref().is_some_and(|error| error.contains("not synchronized")));
        assert!(state.score.lock().expect("lock").controls_at(0).is_empty());
        Ok(())
    }

    #[test]
    fn joining_does_not_seed_a_shared_default() -> Result<()> {
        let dir = tempfile::tempdir()?;
        let blobs = BlobStore::open(dir.path().join("blobs"))?;
        let joining_score = Score::new();
        assert!(seed_default_instrument(&joining_score, &blobs, true)?.is_none());
        assert!(joining_score.instrument()?.is_none());

        let fresh_score = Score::new();
        let hash = seed_default_instrument(&fresh_score, &blobs, false)?
            .expect("fresh session gets the default");
        assert_eq!(fresh_score.instrument()?.expect("instrument").hash, hash);
        Ok(())
    }

    #[test]
    fn rejects_a_restored_zero_sample_rate() -> Result<()> {
        let score = Score::new();
        score.set_sample_rate(0)?;
        let error = validated_sample_rate(&score).expect_err("zero sample rate must fail");
        assert!(error.to_string().contains("greater than zero"));
        Ok(())
    }

    #[test]
    fn final_persist_writes_the_latest_score() -> Result<()> {
        let dir = tempfile::tempdir()?;
        let path = dir.path().join("score.loro");
        let score = Mutex::new(Score::new());
        score.lock().expect("lock").set_control(9, -0.25, 77)?;
        persist_score(&score, &path)?;

        let restored = Score::new();
        restored.import(&std::fs::read(path)?)?;
        let control = restored.controls_at(77).into_iter().find(|control| control.control == 9);
        assert_eq!(control.expect("persisted control").value, -0.25);
        Ok(())
    }

    #[test]
    fn peer_identity_survives_restart() -> Result<()> {
        let dir = tempfile::tempdir()?;
        prepare_state_dir(dir.path())?;
        let first = load_or_create_peer_id(dir.path())?;
        let second = load_or_create_peer_id(dir.path())?;
        assert_eq!(first, second);
        Ok(())
    }

    #[tokio::test]
    async fn hostname_peers_resolve() -> Result<()> {
        let peers = resolve_peers(&["localhost:7648".to_owned()]).await?;
        assert!(!peers.is_empty());
        assert!(peers.iter().all(|peer| peer.port() == 7648));
        Ok(())
    }

    #[tokio::test]
    async fn control_socket_is_private_and_cannot_be_stolen() -> Result<()> {
        let dir = tempfile::tempdir()?;
        prepare_state_dir(dir.path())?;
        let path = dir.path().join("control.sock");
        let listener = bind_control_listener(&path).await?;
        let mode = std::fs::metadata(&path)?.permissions().mode() & 0o777;
        assert_eq!(mode, 0o600);
        let error = bind_control_listener(&path).await.expect_err("live socket must be retained");
        assert!(error.to_string().contains("another daemon"));
        drop(listener);
        Ok(())
    }

    #[test]
    fn state_directory_is_private() -> Result<()> {
        let root = tempfile::tempdir()?;
        let path = root.path().join("state");
        prepare_state_dir(&path)?;
        let mode = std::fs::metadata(path)?.permissions().mode() & 0o777;
        assert_eq!(mode, 0o700);
        Ok(())
    }

    #[tokio::test]
    async fn default_instrument_loads() -> Result<()> {
        Instrument::load(DEFAULT_INSTRUMENT_WAT.as_bytes())?;
        Ok(())
    }
}
