//! The design's end-to-end claim: ship the instrument and the score, never
//! audio, and two peers still hear the exact same thing.
//!
//! Two in-process nodes over loopback (static peers), one publish on node A,
//! then: B converges on the score, fetches the module by hash, follows A's
//! clock, and renders bit-identical samples for the same shared-frame range.

use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use anyhow::Result;
use audio_blob::BlobStore;
use audio_clock::{MonotonicTime, PeerId, ProcessTime, SAMPLE_RATE};
use audio_engine::Renderer;
use audio_net::{Config, NodeHandle};
use audio_score::{ControlValue, Event, Score};

/// Stateless mono instrument: `sample = fract(frame / 1000) * controls[1] +
/// controls[0]`. Pure in the absolute frame, so any peer and any block
/// split renders the same bits.
const RAMP_WAT: &str = r#"
(module
  (memory (export "memory") 2)
  (func (export "sa_abi_version") (result i32) (i32.const 1))
  (func (export "sa_channels") (result i32) (i32.const 1))
  (func (export "sa_controls_ptr") (result i32) (i32.const 0))
  (func (export "sa_out_ptr") (result i32) (i32.const 1024))
  (func (export "sa_render") (param $start i64) (param $n i32) (param $sr i32)
    (local $i i32) (local $phase f64)
    (block $done
      (loop $loop
        (br_if $done (i32.ge_s (local.get $i) (local.get $n)))
        (local.set $phase
          (f64.div
            (f64.convert_i64_s
              (i64.add (local.get $start) (i64.extend_i32_s (local.get $i))))
            (f64.const 1000)))
        (local.set $phase
          (f64.sub (local.get $phase) (f64.floor (local.get $phase))))
        (f32.store
          (i32.add (i32.const 1024) (i32.mul (local.get $i) (i32.const 4)))
          (f32.add
            (f32.mul (f32.demote_f64 (local.get $phase)) (f32.load (i32.const 4)))
            (f32.load (i32.const 0))))
        (local.set $i (i32.add (local.get $i) (i32.const 1)))
        (br $loop))))
)
"#;

struct Peer {
    score: Arc<Mutex<Score>>,
    store: Arc<BlobStore>,
    node: NodeHandle,
    _dir: tempfile::TempDir,
}

async fn spawn_peer(
    id: u64,
    peers: Vec<std::net::SocketAddr>,
    time: &Arc<dyn MonotonicTime>,
) -> Result<Peer> {
    let dir = tempfile::tempdir()?;
    let blobs = Arc::new(BlobStore::open(dir.path())?);
    let score = Arc::new(Mutex::new(Score::new()));
    let node = audio_net::spawn(
        Config {
            peer_id: PeerId(id),
            tcp_bind: "127.0.0.1:0".parse()?,
            udp_bind: "127.0.0.1:0".parse()?,
            peers,
            sample_rate: SAMPLE_RATE,
            time: Arc::clone(time),
        },
        Arc::clone(&score),
        Arc::clone(&blobs),
    )
    .await?;
    Ok(Peer {
        score,
        store: blobs,
        node,
        _dir: dir,
    })
}

/// Poll `condition` until it holds or the deadline blows.
async fn converge(what: &str, mut condition: impl FnMut() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(30);
    while !condition() {
        assert!(Instant::now() < deadline, "timed out waiting for {what}");
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn publish_converges_to_bit_identical_audio() -> Result<()> {
    let time: Arc<dyn MonotonicTime> = Arc::new(ProcessTime::default());
    // Smaller peer id wins leadership: A leads, B follows.
    let a = spawn_peer(1, vec![], &time).await?;
    let b = spawn_peer(2, vec![a.node.tcp_addr], &time).await?;

    // Publish on A only: module bytes into A's store, hash + controls +
    // scheduled events into A's score.
    let hash = a.store.put(RAMP_WAT.as_bytes())?;
    {
        let score = a.score.lock().expect("lock");
        score.set_sample_rate(SAMPLE_RATE)?;
        score.set_instrument(&hash, 0)?;
        score.set_control(0, 0.125)?;
        score.set_control(1, 0.5)?;
        score.schedule(Event {
            at_frame: 2_000,
            control: 1,
            value: 0.25,
        })?;
        score.schedule(Event {
            at_frame: 3_000,
            control: 0,
            value: -0.125,
        })?;
    }

    // B converges: score gossip carries the hash and events, the blob
    // protocol carries the module bytes, ping sampling follows A's clock.
    converge("score to reach B", || {
        let score = b.score.lock().expect("lock");
        score
            .instrument()
            .ok()
            .flatten()
            .is_some_and(|i| i.hash == hash)
            && score.events().len() == 2
    })
    .await;
    converge("module bytes to reach B", || b.store.contains(&hash)).await;
    converge("B to follow A's clock", || {
        b.node.clock().epoch_micros() == a.node.clock().epoch_micros()
    })
    .await;

    // Same shared range, both peers, both from their own local state.
    let mut renderer_a = Renderer::new(Arc::clone(&a.score), Arc::clone(&a.store));
    let mut renderer_b = Renderer::new(Arc::clone(&b.score), Arc::clone(&b.store));
    let frames = 4_801;
    let mut out_a = vec![0.0_f32; frames];
    let mut out_b = vec![0.0_f32; frames];
    renderer_a.render_range(1_000, frames, SAMPLE_RATE, &mut out_a)?;
    renderer_b.render_range(1_000, frames, SAMPLE_RATE, &mut out_b)?;
    let bits_a: Vec<u32> = out_a.iter().map(|s| s.to_bits()).collect();
    let bits_b: Vec<u32> = out_b.iter().map(|s| s.to_bits()).collect();
    assert_eq!(bits_a, bits_b, "peers must render bit-identical audio");
    assert!(
        out_a.iter().any(|&s| s != 0.0),
        "the range actually made sound"
    );

    // The clocks agree on *when* those frames play: within 50 ms.
    let now = time.now_micros();
    let frame_a = a.node.clock().frame_at(now, SAMPLE_RATE);
    let frame_b = b.node.clock().frame_at(now, SAMPLE_RATE);
    let drift_frames = (frame_a - frame_b).unsigned_abs();
    assert!(
        drift_frames < u64::from(SAMPLE_RATE) / 20,
        "clock drift {drift_frames} frames exceeds 50ms"
    );
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn edits_flow_both_ways() -> Result<()> {
    let time: Arc<dyn MonotonicTime> = Arc::new(ProcessTime::default());
    let a = spawn_peer(1, vec![], &time).await?;
    let b = spawn_peer(2, vec![a.node.tcp_addr], &time).await?;

    a.score.lock().expect("lock").set_control(3, 0.75)?;
    b.score.lock().expect("lock").set_control(4, 0.25)?;

    let both = |score: &Arc<Mutex<Score>>| {
        let controls = score.lock().expect("lock").controls();
        controls.contains(&ControlValue {
            control: 3,
            value: 0.75,
        }) && controls.contains(&ControlValue {
            control: 4,
            value: 0.25,
        })
    };
    converge("A to see both controls", || both(&a.score)).await;
    converge("B to see both controls", || both(&b.score)).await;
    Ok(())
}
