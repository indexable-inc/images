//! Deterministic renderer plus a streaming player.
//!
//! The split is the crate's whole point:
//! - [`Renderer::render_range`] is a *pure* function of the shared score and
//!   the requested frame range. Two converged peers render bit-identical
//!   samples for the same range, which is what the e2e suite asserts.
//! - [`Player`] wraps that pure core in real time: a render thread runs a
//!   schedule-ahead lead in front of the shared clock and streams blocks to
//!   a [`rodio::Source`]. Local volume is applied here, at the very edge,
//!   never inside the deterministic core and never in the score.

use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::mpsc::{Receiver, SyncSender, TrySendError};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use anyhow::{Context as _, Result};
use audio_blob::{BlobHash, BlobStore};
use audio_clock::{MonotonicTime, SharedClock};
use audio_instrument::{CONTROL_COUNT, Instrument, MAX_BLOCK_FRAMES};
use audio_score::{ControlValue, Score};
use tracing::{info, warn};

/// Renders any span of the shared timeline from the score, deterministically.
pub struct Renderer {
    score: Arc<Mutex<Score>>,
    store: Arc<BlobStore>,
    loaded: Option<(BlobHash, Instrument)>,
    /// The score's named instrument, loaded and waiting for its activation
    /// frame; promoted to `loaded` when a render reaches `at_frame`.
    staged: Option<Staged>,
}

struct Staged {
    hash: BlobHash,
    instrument: Instrument,
    at_frame: u64,
}

impl Renderer {
    /// A renderer over a shared score and blob store.
    #[must_use]
    pub const fn new(score: Arc<Mutex<Score>>, blobs: Arc<BlobStore>) -> Self {
        Self {
            score,
            store: blobs,
            loaded: None,
            staged: None,
        }
    }

    /// Channel count of the loaded instrument, or 1 before any loads.
    #[must_use]
    pub fn channels(&self) -> u32 {
        self.loaded
            .as_ref()
            .map_or(1, |(_, instrument)| instrument.channels())
    }

    /// (Re)load the instrument when the score names a module whose bytes we
    /// hold and it differs from the loaded one. The new module is *staged*,
    /// not swapped in: the previous instrument keeps playing until a render
    /// reaches the score's activation frame, so every peer switches at the
    /// same shared frame no matter when its bytes arrived. Returns whether
    /// an instrument is loaded or staged after the refresh.
    ///
    /// # Errors
    /// Fails when the named module bytes are present but invalid.
    ///
    /// # Panics
    /// Panics when the score mutex is poisoned.
    pub fn refresh(&mut self) -> Result<bool> {
        let wanted = {
            let score = self.score.lock().expect("score lock");
            score.instrument()?
        };
        let Some(wanted) = wanted else {
            return Ok(self.loaded.is_some());
        };
        if self
            .loaded
            .as_ref()
            .is_some_and(|(hash, _)| *hash == wanted.hash)
        {
            self.staged = None;
            return Ok(true);
        }
        if let Some(staged) = &mut self.staged
            && staged.hash == wanted.hash
        {
            staged.at_frame = wanted.at_frame;
            return Ok(true);
        }
        let Some(bytes) = self.store.get(&wanted.hash)? else {
            // Bytes still in flight; keep playing the previous module.
            return Ok(self.loaded.is_some());
        };
        let instrument =
            Instrument::load(&bytes).with_context(|| format!("load instrument {}", wanted.hash))?;
        info!(hash = %wanted.hash, at_frame = wanted.at_frame, "instrument staged");
        self.staged = Some(Staged {
            hash: wanted.hash,
            instrument,
            at_frame: wanted.at_frame,
        });
        Ok(true)
    }

    /// Swap in the staged instrument once `frame` reaches its activation
    /// frame.
    fn promote_due(&mut self, frame: u64) {
        if self
            .staged
            .as_ref()
            .is_some_and(|staged| staged.at_frame <= frame)
        {
            let staged = self.staged.take().expect("staged instrument present");
            info!(hash = %staged.hash, at_frame = staged.at_frame, "instrument activated");
            self.loaded = Some((staged.hash, staged.instrument));
        }
    }

    /// The next staged activation strictly after `frame`, if any.
    fn next_switch_after(&self, frame: u64) -> Option<u64> {
        self.staged
            .as_ref()
            .map(|staged| staged.at_frame)
            .filter(|&at_frame| at_frame > frame)
    }

    /// Render shared-timeline frames `start_frame .. start_frame + frames`
    /// into `out` (interleaved, `frames * channels()` samples).
    ///
    /// Pure with respect to `(score, start_frame, frames)`: control state is
    /// rebuilt from the score every call and events split blocks at their
    /// exact frame, so any block partitioning yields identical bits.
    ///
    /// # Errors
    /// Fails when `out` is missized or the instrument traps.
    ///
    /// # Panics
    /// Panics when the score mutex is poisoned.
    pub fn render_range(
        &mut self,
        start_frame: u64,
        frames: usize,
        sample_rate: u32,
        out: &mut [f32],
    ) -> Result<()> {
        self.refresh()?;
        self.promote_due(start_frame);
        if self.loaded.is_none() && self.staged.is_none() {
            out.fill(0.0);
            return Ok(());
        }
        let channels = self.channels() as usize;
        anyhow::ensure!(
            out.len() == frames * channels,
            "out has {} samples, range needs {}",
            out.len(),
            frames * channels
        );

        // Control state at `start_frame`: base controls, then every event at
        // or before it, in the deterministic event order.
        let (controls, events) = {
            let score = self.score.lock().expect("score lock");
            (score.controls(), score.events())
        };
        let mut state = [0.0_f32; CONTROL_COUNT];
        for ControlValue { control, value } in controls {
            if let Some(slot) = state.get_mut(control as usize) {
                *slot = value;
            }
        }
        let mut pending = events.iter().peekable();
        while let Some(event) = pending.next_if(|event| event.at_frame <= start_frame) {
            if let Some(slot) = state.get_mut(event.control as usize) {
                *slot = event.value;
            }
        }

        // Walk the range, splitting at event frames, staged instrument
        // activation frames, and the ABI block cap.
        let end_frame = start_frame + frames as u64;
        let mut frame = start_frame;
        let mut cursor = 0;
        while frame < end_frame {
            self.promote_due(frame);
            let next_event = pending
                .peek()
                .map_or(end_frame, |event| event.at_frame.min(end_frame));
            let next_switch = self
                .next_switch_after(frame)
                .map_or(end_frame, |at_frame| at_frame.min(end_frame));
            let block_end = next_event
                .min(next_switch)
                .max(frame + 1)
                .min(frame + MAX_BLOCK_FRAMES as u64);
            let block_frames = usize::try_from(block_end - frame).expect("block fits usize");
            let samples = block_frames * channels;
            match &mut self.loaded {
                Some((_, instrument)) => render_block(
                    instrument,
                    frame,
                    block_frames,
                    sample_rate,
                    &state,
                    &mut out[cursor..cursor + samples],
                    channels,
                )?,
                // Silence until the first instrument activates.
                None => out[cursor..cursor + samples].fill(0.0),
            }
            frame = block_end;
            cursor += samples;
            while let Some(event) = pending.next_if(|event| event.at_frame <= frame) {
                if let Some(slot) = state.get_mut(event.control as usize) {
                    *slot = event.value;
                }
            }
        }
        Ok(())
    }
}

/// Render one block, adapting the instrument's native channel count to the
/// range's layout when a mid-range switch changes it: mono duplicates
/// outward, extra channels drop. Deterministic either way, so converged
/// peers still agree bit-for-bit.
fn render_block(
    instrument: &mut Instrument,
    frame: u64,
    block_frames: usize,
    sample_rate: u32,
    state: &[f32; CONTROL_COUNT],
    out: &mut [f32],
    out_channels: usize,
) -> Result<()> {
    let native = instrument.channels() as usize;
    if native == out_channels {
        return instrument.render(frame, block_frames, sample_rate, state, out);
    }
    let mut scratch = vec![0.0_f32; block_frames * native];
    instrument.render(frame, block_frames, sample_rate, state, &mut scratch)?;
    for (chunk, source) in out
        .chunks_exact_mut(out_channels)
        .zip(scratch.chunks_exact(native))
    {
        for (index, slot) in chunk.iter_mut().enumerate() {
            *slot = source[index.min(native - 1)];
        }
    }
    Ok(())
}

/// Local listener volume: linear gain plus a mute flag. Lives outside the
/// score on purpose; one listener's volume never reaches the network.
#[derive(Debug, Clone, Default)]
pub struct Volume {
    inner: Arc<VolumeState>,
}

#[derive(Debug)]
struct VolumeState {
    gain_bits: AtomicU32,
    muted: AtomicBool,
}

impl Default for VolumeState {
    fn default() -> Self {
        Self {
            gain_bits: AtomicU32::new(1.0_f32.to_bits()),
            muted: AtomicBool::new(false),
        }
    }
}

impl Volume {
    /// Current gain, `0.0..=2.0`, ignoring mute.
    #[must_use]
    pub fn gain(&self) -> f32 {
        f32::from_bits(self.inner.gain_bits.load(Ordering::Relaxed))
    }

    /// Set the gain, clamped to `0.0..=2.0`.
    pub fn set_gain(&self, gain: f32) {
        let gain = gain.clamp(0.0, 2.0);
        self.inner
            .gain_bits
            .store(gain.to_bits(), Ordering::Relaxed);
    }

    /// Nudge the gain by `delta` (e.g. `0.1` / `-0.1` for menu steps).
    pub fn step(&self, delta: f32) {
        self.set_gain(self.gain() + delta);
    }

    /// Whether output is muted.
    #[must_use]
    pub fn muted(&self) -> bool {
        self.inner.muted.load(Ordering::Relaxed)
    }

    /// Set or clear mute; the gain is remembered across mutes.
    pub fn set_muted(&self, muted: bool) {
        self.inner.muted.store(muted, Ordering::Relaxed);
    }

    /// The factor applied to samples right now.
    #[must_use]
    pub fn effective(&self) -> f32 {
        if self.muted() { 0.0 } else { self.gain() }
    }
}

/// Frames per streamed block; ~21 ms at 48 kHz.
const BLOCK_FRAMES: usize = 1024;
/// Blocks buffered between the render thread and the audio callback. The
/// bounded channel doubles as pacing: the renderer stays exactly this far
/// ahead of playback.
const BLOCK_BUFFER: usize = 8;
/// Resync when the render position drifts this far from the shared clock.
const RESYNC_MICROS: i64 = 250_000;

/// A real-time player around a [`Renderer`]: render thread + audio source.
pub struct Player {
    stop: Arc<AtomicBool>,
    thread: Option<std::thread::JoinHandle<()>>,
}

impl Player {
    /// Start rendering ahead of `clock` and return the source to feed an
    /// output mixer. `clock` is a snapshot getter so the player always
    /// chases the freshest network estimate.
    /// # Panics
    /// Panics when the render thread cannot be spawned.
    #[must_use]
    pub fn spawn(
        mut renderer: Renderer,
        clock: impl Fn() -> SharedClock + Send + 'static,
        time: Arc<dyn MonotonicTime>,
        sample_rate: u32,
        volume: Volume,
    ) -> PlayerSpawn {
        let stop = Arc::new(AtomicBool::new(false));
        let (sender, receiver) = std::sync::mpsc::sync_channel(BLOCK_BUFFER);
        let thread = std::thread::Builder::new()
            .name("shared-audio-render".into())
            .spawn({
                let stop = Arc::clone(&stop);
                move || {
                    render_loop(
                        &mut renderer,
                        &clock,
                        time.as_ref(),
                        sample_rate,
                        &sender,
                        &stop,
                    )
                }
            })
            .expect("spawn render thread");
        let source = TimelineSource {
            receiver,
            block: Vec::new(),
            cursor: 0,
            sample_rate,
            volume,
        };
        PlayerSpawn {
            player: Self {
                stop,
                thread: Some(thread),
            },
            source,
        }
    }
}

/// [`Player::spawn`] result.
pub struct PlayerSpawn {
    /// Handle whose drop stops the render thread.
    pub player: Player,
    /// Source to feed an output mixer.
    pub source: TimelineSource,
}

impl Drop for Player {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

/// Render blocks just ahead of the shared clock. The bounded channel paces
/// the loop; a resync (peer joined, leader changed, output stalled) logs
/// loudly instead of drifting silently.
fn render_loop(
    renderer: &mut Renderer,
    clock: &impl Fn() -> SharedClock,
    time: &dyn MonotonicTime,
    sample_rate: u32,
    sender: &SyncSender<Vec<f32>>,
    stop: &AtomicBool,
) {
    let mut frame = playhead(&clock(), time, sample_rate);
    while !stop.load(Ordering::Relaxed) {
        let now = clock();
        let due_micros = now.local_micros_of_frame(frame, sample_rate);
        let lead = due_micros - i64::try_from(time.now_micros()).expect("monotonic micros fit i64");
        if lead.abs() > RESYNC_MICROS {
            let target = playhead(&now, time, sample_rate);
            warn!(
                drift_micros = lead,
                from = frame,
                to = target,
                "resyncing to shared clock"
            );
            frame = target;
        }

        let mut block = vec![0.0_f32; BLOCK_FRAMES * 2];
        if let Err(error) = render_stereo(
            renderer,
            frame.max(0).unsigned_abs(),
            sample_rate,
            &mut block,
        ) {
            warn!(%error, "render failed; emitting silence");
            block.fill(0.0);
        }
        frame += i64::try_from(BLOCK_FRAMES).expect("block fits i64");

        // Blocking send paces us to real time; on stop the receiver is
        // dropped and send errors us out.
        let mut outgoing = block;
        loop {
            match sender.try_send(outgoing) {
                Ok(()) => break,
                Err(TrySendError::Full(back)) => {
                    if stop.load(Ordering::Relaxed) {
                        return;
                    }
                    outgoing = back;
                    std::thread::sleep(Duration::from_millis(5));
                }
                Err(TrySendError::Disconnected(_)) => return,
            }
        }
    }
}

/// Where the playhead should be right now, plus the buffer lead.
fn playhead(clock: &SharedClock, time: &dyn MonotonicTime, sample_rate: u32) -> i64 {
    let lead = i64::try_from(BLOCK_FRAMES * BLOCK_BUFFER).expect("lead fits i64");
    clock.frame_at(time.now_micros(), sample_rate) + lead
}

/// Render one stereo block, adapting the instrument's channel count (mono
/// duplicates into both ears). Local adaptation only; the deterministic
/// cross-peer contract is [`Renderer::render_range`] itself.
fn render_stereo(
    renderer: &mut Renderer,
    start_frame: u64,
    sample_rate: u32,
    out: &mut [f32],
) -> Result<()> {
    let frames = out.len() / 2;
    renderer.refresh()?;
    renderer.promote_due(start_frame);
    if renderer.channels() == 2 {
        return renderer.render_range(start_frame, frames, sample_rate, out);
    }
    let mut mono = vec![0.0_f32; frames];
    renderer.render_range(start_frame, frames, sample_rate, &mut mono)?;
    for (pair, sample) in out.chunks_exact_mut(2).zip(&mono) {
        pair[0] = *sample;
        pair[1] = *sample;
    }
    Ok(())
}

/// The rodio source end of a [`Player`]: interleaved stereo `f32`.
pub struct TimelineSource {
    receiver: Receiver<Vec<f32>>,
    block: Vec<f32>,
    cursor: usize,
    sample_rate: u32,
    volume: Volume,
}

impl Iterator for TimelineSource {
    type Item = f32;

    fn next(&mut self) -> Option<f32> {
        if self.cursor >= self.block.len() {
            match self.receiver.recv_timeout(Duration::from_millis(100)) {
                Ok(block) => {
                    self.block = block;
                    self.cursor = 0;
                }
                Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {
                    // Underrun: keep the stream alive with silence and let
                    // the render loop resync.
                    return Some(0.0);
                }
                Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => return None,
            }
        }
        let sample = self.block[self.cursor] * self.volume.effective();
        self.cursor += 1;
        Some(sample)
    }
}

impl rodio::Source for TimelineSource {
    fn current_span_len(&self) -> Option<usize> {
        None
    }

    fn channels(&self) -> rodio::ChannelCount {
        rodio::ChannelCount::new(2).expect("2 != 0")
    }

    fn sample_rate(&self) -> rodio::SampleRate {
        rodio::SampleRate::new(self.sample_rate.max(1)).expect("nonzero")
    }

    fn total_duration(&self) -> Option<Duration> {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use audio_score::Event;

    /// Mono instrument that outputs controls[0] on every frame, so a sample
    /// value *is* the control state at that frame.
    const CONST_WAT: &str = r#"
(module
  (memory (export "memory") 2)
  (func (export "sa_abi_version") (result i32) (i32.const 1))
  (func (export "sa_channels") (result i32) (i32.const 1))
  (func (export "sa_controls_ptr") (result i32) (i32.const 0))
  (func (export "sa_out_ptr") (result i32) (i32.const 256))
  (func (export "sa_render") (param $start i64) (param $n i32) (param $sr i32)
    (local $i i32)
    (block $done
      (loop $loop
        (br_if $done (i32.ge_s (local.get $i) (local.get $n)))
        (f32.store
          (i32.add (i32.const 256) (i32.mul (local.get $i) (i32.const 4)))
          (f32.load (i32.const 0)))
        (local.set $i (i32.add (local.get $i) (i32.const 1)))
        (br $loop))))
)
"#;

    /// Mono instrument that outputs controls[0] + 1.0 on every frame, so a
    /// switch away from [`CONST_WAT`] is visible in the samples.
    const SHIFT_WAT: &str = r#"
(module
  (memory (export "memory") 2)
  (func (export "sa_abi_version") (result i32) (i32.const 1))
  (func (export "sa_channels") (result i32) (i32.const 1))
  (func (export "sa_controls_ptr") (result i32) (i32.const 0))
  (func (export "sa_out_ptr") (result i32) (i32.const 256))
  (func (export "sa_render") (param $start i64) (param $n i32) (param $sr i32)
    (local $i i32)
    (block $done
      (loop $loop
        (br_if $done (i32.ge_s (local.get $i) (local.get $n)))
        (f32.store
          (i32.add (i32.const 256) (i32.mul (local.get $i) (i32.const 4)))
          (f32.add (f32.load (i32.const 0)) (f32.const 1)))
        (local.set $i (i32.add (local.get $i) (i32.const 1)))
        (br $loop))))
)
"#;

    struct Fixture {
        score: Arc<Mutex<Score>>,
        renderer: Renderer,
        _dir: tempfile::TempDir,
    }

    fn fixture() -> Result<Fixture> {
        let dir = tempfile::tempdir()?;
        let blobs = Arc::new(BlobStore::open(dir.path())?);
        let hash = blobs.put(CONST_WAT.as_bytes())?;
        let score = Arc::new(Mutex::new(Score::new()));
        {
            let score = score.lock().expect("lock");
            score.set_instrument(&hash, 0)?;
            score.set_control(0, 0.25)?;
        }
        let renderer = Renderer::new(Arc::clone(&score), blobs);
        Ok(Fixture {
            score,
            renderer,
            _dir: dir,
        })
    }

    #[test]
    fn events_apply_at_exact_frames() -> Result<()> {
        let Fixture {
            score,
            mut renderer,
            _dir,
        } = fixture()?;
        score.lock().expect("lock").schedule(Event {
            at_frame: 100,
            control: 0,
            value: 0.75,
        })?;

        let mut out = vec![0.0; 200];
        renderer.render_range(0, 200, 48_000, &mut out)?;
        assert!(
            out[..100]
                .iter()
                .all(|&sample| (sample - 0.25).abs() < f32::EPSILON)
        );
        assert!(
            out[100..]
                .iter()
                .all(|&sample| (sample - 0.75).abs() < f32::EPSILON)
        );
        Ok(())
    }

    #[test]
    fn any_block_split_is_bit_exact() -> Result<()> {
        let Fixture {
            score,
            renderer: mut a,
            _dir,
        } = fixture()?;
        score.lock().expect("lock").schedule(Event {
            at_frame: 37,
            control: 0,
            value: 0.5,
        })?;
        let Fixture {
            score: score_b,
            renderer: mut b,
            _dir: _dir_b,
        } = fixture()?;
        score_b.lock().expect("lock").schedule(Event {
            at_frame: 37,
            control: 0,
            value: 0.5,
        })?;

        let mut whole = vec![0.0; 512];
        a.render_range(0, 512, 48_000, &mut whole)?;

        let mut pieces = vec![0.0; 512];
        b.render_range(0, 13, 48_000, &mut pieces[..13])?;
        b.render_range(13, 100, 48_000, &mut pieces[13..113])?;
        b.render_range(113, 399, 48_000, &mut pieces[113..])?;

        let whole: Vec<u32> = whole.iter().map(|sample| sample.to_bits()).collect();
        let pieces: Vec<u32> = pieces.iter().map(|sample| sample.to_bits()).collect();
        assert_eq!(whole, pieces);
        Ok(())
    }

    #[test]
    fn instrument_switches_at_its_activation_frame() -> Result<()> {
        let dir = tempfile::tempdir()?;
        let blobs = Arc::new(BlobStore::open(dir.path())?);
        let hash_a = blobs.put(CONST_WAT.as_bytes())?;
        let hash_b = blobs.put(SHIFT_WAT.as_bytes())?;
        let score = Arc::new(Mutex::new(Score::new()));
        {
            let score = score.lock().expect("lock");
            score.set_instrument(&hash_a, 0)?;
            score.set_control(0, 0.25)?;
        }
        let mut renderer = Renderer::new(Arc::clone(&score), Arc::clone(&blobs));
        let mut out = vec![0.0; 8];
        renderer.render_range(0, 8, 48_000, &mut out)?;
        assert!(
            out.iter()
                .all(|&sample| (sample - 0.25).abs() < f32::EPSILON)
        );

        // Publish the successor: bytes already held, but it must not sound
        // before its activation frame.
        score.lock().expect("lock").set_instrument(&hash_b, 100)?;
        let mut out = vec![0.0; 200];
        renderer.render_range(0, 200, 48_000, &mut out)?;
        assert!(
            out[..100]
                .iter()
                .all(|&sample| (sample - 0.25).abs() < f32::EPSILON)
        );
        assert!(
            out[100..]
                .iter()
                .all(|&sample| (sample - 1.25).abs() < f32::EPSILON)
        );

        // A peer that never held the old module stays silent until the
        // shared switch point instead of jumping ahead.
        let mut fresh = Renderer::new(Arc::clone(&score), blobs);
        let mut out = vec![1.0; 200];
        fresh.render_range(0, 200, 48_000, &mut out)?;
        assert!(out[..100].iter().all(|&sample| sample == 0.0));
        assert!(
            out[100..]
                .iter()
                .all(|&sample| (sample - 1.25).abs() < f32::EPSILON)
        );
        Ok(())
    }

    #[test]
    fn silence_before_any_instrument() -> Result<()> {
        let dir = tempfile::tempdir()?;
        let blobs = Arc::new(BlobStore::open(dir.path())?);
        let score = Arc::new(Mutex::new(Score::new()));
        let mut renderer = Renderer::new(score, blobs);
        let mut out = vec![1.0; 64];
        renderer.render_range(0, 64, 48_000, &mut out)?;
        assert!(out.iter().all(|&sample| sample == 0.0));
        Ok(())
    }

    #[test]
    fn volume_scales_playback_but_never_rendering() -> Result<()> {
        let Fixture {
            score: _score,
            mut renderer,
            _dir,
        } = fixture()?;
        let volume = Volume::default();
        volume.set_gain(0.0);
        let mut out = vec![0.0; 16];
        renderer.render_range(0, 16, 48_000, &mut out)?;
        // Deterministic core ignores volume entirely.
        assert!(
            out.iter()
                .all(|&sample| (sample - 0.25).abs() < f32::EPSILON)
        );
        // The playback edge applies it.
        assert!((volume.effective() - 0.0).abs() < f32::EPSILON);
        volume.set_gain(0.5);
        volume.set_muted(true);
        assert!((volume.effective() - 0.0).abs() < f32::EPSILON);
        volume.set_muted(false);
        assert!((volume.effective() - 0.5).abs() < f32::EPSILON);
        Ok(())
    }
}
