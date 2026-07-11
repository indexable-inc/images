//! The `shared-audio daemon`: joins the LAN session, plays the shared
//! timeline, and serves the control socket. Designed to run under
//! launchd/systemd (see this repo's `homeModules.portable-services`).

use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use anyhow::{Context as _, Result};
use audio_blob::BlobStore;
use audio_clock::{MonotonicTime, PeerId, ProcessTime, SAMPLE_RATE};
use audio_engine::{Player, Renderer, Volume};
use audio_instrument::Instrument;
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

#[derive(Debug, clap::Args)]
pub struct Opts {
    /// Static peer addresses to gossip with (repeatable). LAN discovery is
    /// deliberately injected: point peers at each other explicitly, from
    /// config, or from a future mDNS feeder.
    #[arg(long = "peer")]
    pub peers: Vec<std::net::SocketAddr>,
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
}

async fn run_async(opts: Opts) -> Result<()> {
    let state_dir = opts.state_dir.unwrap_or_else(control::state_dir);
    std::fs::create_dir_all(&state_dir)
        .with_context(|| format!("create state dir {}", state_dir.display()))?;
    let store = Arc::new(BlobStore::open(state_dir.join("blobs"))?);

    let score_path = state_dir.join("score.loro");
    let score = Score::new();
    if let Ok(snapshot) = std::fs::read(&score_path) {
        score.import(&snapshot).context("import persisted score")?;
        info!(path = %score_path.display(), "restored score snapshot");
    }
    if score.instrument()?.is_none() {
        let hash = store.put(DEFAULT_INSTRUMENT_WAT.as_bytes())?;
        score.set_instrument(&hash, 0)?;
        info!(%hash, "seeded default instrument");
    }
    let sample_rate = score.sample_rate().unwrap_or(SAMPLE_RATE);
    score.set_sample_rate(sample_rate)?;
    let score = Arc::new(Mutex::new(score));

    let peer_id = PeerId::random();
    let time: Arc<dyn MonotonicTime> = Arc::new(ProcessTime::default());
    let node = Arc::new(
        audio_net::spawn(
            audio_net::Config {
                peer_id,
                tcp_bind: opts.tcp_bind,
                udp_bind: opts.udp_bind,
                peers: opts.peers,
                sample_rate,
                time: Arc::clone(&time),
            },
            Arc::clone(&score),
            Arc::clone(&store),
        )
        .await?,
    );
    info!(peer = peer_id.0, tcp = %node.tcp_addr, udp = %node.udp_addr, "node up");

    let volume = Volume::default();
    let _audio = if opts.no_audio {
        None
    } else {
        Some(start_audio(&score, &store, &node, &time, sample_rate, &volume)?)
    };

    let state = Arc::new(State {
        score: Arc::clone(&score),
        store,
        volume,
        node,
        time,
        peer_id,
        sample_rate,
    });

    tokio::spawn(persist_loop(Arc::clone(&score), score_path));

    let socket_path = control::socket_path();
    if let Some(parent) = socket_path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let _ = std::fs::remove_file(&socket_path);
    let listener = UnixListener::bind(&socket_path)
        .with_context(|| format!("bind control socket {}", socket_path.display()))?;
    info!(socket = %socket_path.display(), "control socket ready");

    loop {
        tokio::select! {
            accepted = listener.accept() => {
                let (stream, _) = accepted?;
                tokio::spawn(serve_client(stream, Arc::clone(&state)));
            }
            _ = shutdown_signal() => break,
        }
    }
    let _ = std::fs::remove_file(&socket_path);
    info!("daemon stopped");
    Ok(())
}

/// Open the default output device and start the schedule-ahead player.
fn start_audio(
    score: &Arc<Mutex<Score>>,
    store: &Arc<BlobStore>,
    node: &Arc<NodeHandle>,
    time: &Arc<dyn MonotonicTime>,
    sample_rate: u32,
    volume: &Volume,
) -> Result<(Player, rodio::MixerDeviceSink, rodio::Player)> {
    let renderer = Renderer::new(Arc::clone(score), Arc::clone(store));
    let clock_node = Arc::clone(node);
    let (player, source) = Player::spawn(
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
    Ok((player, device, sink))
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
        let (version, snapshot) = {
            let score = score.lock().expect("score lock");
            (score.version(), score.export_snapshot())
        };
        if version == persisted {
            continue;
        }
        let snapshot = match snapshot {
            Ok(bytes) => bytes,
            Err(error) => {
                warn!(%error, "score snapshot export failed");
                continue;
            }
        };
        let tmp = path.with_extension("loro.tmp");
        let result = std::fs::write(&tmp, &snapshot).and_then(|()| std::fs::rename(&tmp, &path));
        match result {
            Ok(()) => persisted = version,
            Err(error) => warn!(%error, path = %path.display(), "score persist failed"),
        }
    }
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
            let score = state.score.lock().expect("score lock");
            Ok(Response {
                ok: true,
                error: None,
                status: Some(Status {
                    peer_id: state.peer_id.0,
                    tcp_addr: state.node.tcp_addr.to_string(),
                    udp_addr: state.node.udp_addr.to_string(),
                    sample_rate: state.sample_rate,
                    frame_now: clock.frame_at(state.time.now_micros(), state.sample_rate),
                    epoch_micros: clock.epoch_micros(),
                    gain: state.volume.gain(),
                    muted: state.volume.muted(),
                    instrument: score.instrument()?.map(|i| i.hash.to_string()),
                    controls: score.controls(),
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
            // Validate before it can reach any peer's score.
            Instrument::load(&bytes).context("instrument rejected")?;
            let hash = state.store.put(&bytes)?;
            let at_frame = at_frame.unwrap_or_else(|| one_second_out(state));
            state
                .score
                .lock()
                .expect("score lock")
                .set_instrument(&hash, at_frame)?;
            info!(%hash, at_frame, "instrument published");
            Ok(Response::ok())
        }
        Request::SetControl { control, value } => {
            state.score.lock().expect("score lock").set_control(control, value)?;
            Ok(Response::ok())
        }
        Request::Schedule { at_frame, control, value } => {
            state
                .score
                .lock()
                .expect("score lock")
                .schedule(audio_score::Event { at_frame, control, value })?;
            Ok(Response::ok())
        }
    }
}

/// The shared frame one second from now; the default publish switch point.
fn one_second_out(state: &State) -> u64 {
    let now = state.node.clock().frame_at(state.time.now_micros(), state.sample_rate);
    (now + i64::from(state.sample_rate)).max(0).unsigned_abs()
}

#[cfg(test)]
mod tests {
    use super::*;

    async fn test_state() -> Result<(Arc<State>, tempfile::TempDir)> {
        let dir = tempfile::tempdir()?;
        let store = Arc::new(BlobStore::open(dir.path().join("blobs"))?);
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
                Arc::clone(&store),
            )
            .await?,
        );
        Ok((
            Arc::new(State {
                score,
                store,
                volume: Volume::default(),
                node,
                time,
                peer_id,
                sample_rate: SAMPLE_RATE,
            }),
            dir,
        ))
    }

    #[tokio::test]
    async fn volume_requests_never_touch_the_score() -> Result<()> {
        let (state, _dir) = test_state().await?;
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
        let (state, _dir) = test_state().await?;
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
    async fn default_instrument_loads() -> Result<()> {
        Instrument::load(DEFAULT_INSTRUMENT_WAT.as_bytes())?;
        Ok(())
    }
}
