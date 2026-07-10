//! Composable send-side audio enhancement stages.
//!
//! Everything here operates on 48 kHz mono `f32` PCM in the Web Audio
//! convention (samples in `-1.0..=1.0`) and is pure computation: no I/O, no
//! threads, no platform dependencies. The same crate compiles for native
//! targets and `wasm32-unknown-unknown`, so one implementation serves both
//! native pipelines and browser capture paths (via a thin wasm-bindgen
//! wrapper in the consuming repo).
//!
//! The extension point is [`EnhancementStage`]: frames in, frames out, fixed
//! sample-rate contract. [`Denoiser`] (an `RNNoise` port) is the first backend;
//! an accelerated model (e.g. a DeepFilterNet-class network on CoreML/Metal
//! or CUDA) can implement the same trait without any API change. Stages
//! compose with [`Pipeline`].

use std::collections::VecDeque;

/// Sample rate every stage in this crate is calibrated for.
///
/// `RNNoise`'s model and band layout assume 48 kHz; feeding other rates
/// degrades quality silently, so callers must resample (or pin their audio
/// context) to this rate.
pub const SAMPLE_RATE_HZ: u32 = 48_000;

/// One in-place enhancement stage over 48 kHz mono `f32` PCM.
///
/// Contract:
/// - Frames are mono, 48 kHz ([`SAMPLE_RATE_HZ`]), samples in `-1.0..=1.0`.
/// - Any frame length is accepted; the stage returns exactly as many samples
///   as it was given (buffering internally and, if unavoidable, delaying the
///   stream by a fixed amount of leading silence).
/// - A stage instance carries state across frames and therefore handles
///   exactly one audio stream.
pub trait EnhancementStage {
    /// Rewrite `frame` in place with the enhanced samples.
    fn process_frame(&mut self, frame: &mut [f32]);
}

/// Samples per internal `RNNoise` chunk: 480 (10 ms at 48 kHz). Frames that
/// are multiples of this incur zero added latency from [`Denoiser`].
pub const CHUNK_SAMPLES: usize = nnnoiseless::DenoiseState::FRAME_SIZE;

/// `RNNoise` operates on `i16`-range floats; Web Audio uses `-1.0..=1.0`.
const PCM_SCALE: f32 = 32_768.0;

/// Streaming `RNNoise` noise suppressor — the first [`EnhancementStage`]
/// backend (pure-Rust CPU inference via [`nnnoiseless`]).
///
/// `RNNoise` works in 480-sample chunks; this type chunks internally so
/// callers can feed frames of any length. When every input frame is a
/// multiple of 480 samples (e.g. a 20 ms / 960-sample capture path) the
/// output is sample-aligned with the input and no latency is added.
/// Otherwise a one-time 480-sample (10 ms) delay of leading silence is
/// inserted at the start of the stream so output frames can always be
/// filled completely.
pub struct Denoiser {
    state: Box<nnnoiseless::DenoiseState<'static>>,
    /// Raw input samples not yet grouped into a full `RNNoise` chunk.
    pending: VecDeque<f32>,
    /// Denoised samples not yet handed back to the caller.
    processed: VecDeque<f32>,
    /// Whether the one-time start-of-stream delay has been inserted (only
    /// happens when the caller feeds frames that are not multiples of
    /// [`CHUNK_SAMPLES`]).
    primed: bool,
}

impl Denoiser {
    /// Create a denoiser using the built-in `RNNoise` model.
    #[must_use]
    pub fn new() -> Self {
        Self {
            state: nnnoiseless::DenoiseState::new(),
            pending: VecDeque::new(),
            processed: VecDeque::new(),
            primed: false,
        }
    }
}

impl Default for Denoiser {
    fn default() -> Self {
        Self::new()
    }
}

impl EnhancementStage for Denoiser {
    fn process_frame(&mut self, frame: &mut [f32]) {
        self.pending.extend(frame.iter().copied());

        let mut input = [0.0_f32; CHUNK_SAMPLES];
        let mut output = [0.0_f32; CHUNK_SAMPLES];
        while self.pending.len() >= CHUNK_SAMPLES {
            for slot in &mut input {
                // `pop_front` cannot fail: the loop condition guarantees at
                // least CHUNK_SAMPLES queued samples.
                *slot = self.pending.pop_front().unwrap_or(0.0) * PCM_SCALE;
            }
            self.state.process_frame(&mut output, &input);
            self.processed
                .extend(output.iter().map(|s| (s / PCM_SCALE).clamp(-1.0, 1.0)));
        }

        if self.processed.len() < frame.len() {
            // Misaligned caller: insert the one-time start-of-stream delay.
            // The deficit is always < CHUNK_SAMPLES (it equals the leftover
            // in `pending`), so one chunk of leading silence keeps every
            // future frame fully covered.
            debug_assert!(!self.primed, "denoiser output underflow after priming");
            self.primed = true;
            for _ in 0..CHUNK_SAMPLES {
                self.processed.push_front(0.0);
            }
        }

        for slot in frame.iter_mut() {
            *slot = self.processed.pop_front().unwrap_or(0.0);
        }
    }
}

/// Runs a sequence of [`EnhancementStage`]s in order over each frame.
///
/// Today that is just denoising; an AGC or a second model slots in as
/// another stage without touching callers.
#[derive(Default)]
pub struct Pipeline {
    stages: Vec<Box<dyn EnhancementStage>>,
}

impl Pipeline {
    /// An empty pipeline (passes audio through untouched).
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Append a stage; stages run in insertion order.
    #[must_use]
    pub fn with_stage(mut self, stage: Box<dyn EnhancementStage>) -> Self {
        self.stages.push(stage);
        self
    }
}

impl EnhancementStage for Pipeline {
    fn process_frame(&mut self, frame: &mut [f32]) {
        for stage in &mut self.stages {
            stage.process_frame(frame);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{CHUNK_SAMPLES, Denoiser, EnhancementStage, Pipeline};

    /// Deterministic white-ish noise in `-amplitude..amplitude` (xorshift32;
    /// no rand dependency so tests stay reproducible everywhere).
    #[expect(
        clippy::cast_precision_loss,
        reason = "only 24 bits kept, exactly representable in f32"
    )]
    fn noise(len: usize, amplitude: f32, seed: u32) -> Vec<f32> {
        let mut state = seed;
        (0..len)
            .map(|_| {
                state ^= state << 13;
                state ^= state >> 17;
                state ^= state << 5;
                // Keep 24 bits so the u32 -> f32 conversion is exact.
                let unit = (state >> 8) as f32 / (1 << 24) as f32;
                unit.mul_add(2.0, -1.0) * amplitude
            })
            .collect()
    }

    fn energy(samples: &[f32]) -> f64 {
        samples.iter().map(|s| f64::from(*s) * f64::from(*s)).sum()
    }

    #[test]
    fn denoising_white_noise_reduces_energy() {
        let mut denoiser = Denoiser::new();
        // 300 chunk-aligned frames of 20 ms (960 samples), like a webview
        // capture path. Measure the final second after the model has adapted
        // to the noise floor. Pure white noise is a worst case for RNNoise
        // (there is no speech to contrast against, so it never fully gates);
        // assert a clear-but-honest attenuation, not near-silence. The input
        // is deterministic, so the measured ratio (~0.73 with nnnoiseless
        // 0.5.2) is stable run to run.
        let frame_len = 2 * CHUNK_SAMPLES;
        let frames = 300;
        let input = noise(frames * frame_len, 0.1, 0x1234_5678);
        let mut input_tail_energy = 0.0;
        let mut output_tail_energy = 0.0;
        for (index, chunk) in input.chunks_exact(frame_len).enumerate() {
            let mut frame = chunk.to_vec();
            denoiser.process_frame(&mut frame);
            if index >= frames - 50 {
                input_tail_energy += energy(chunk);
                output_tail_energy += energy(&frame);
            }
        }
        assert!(input_tail_energy > 0.0);
        assert!(
            output_tail_energy < input_tail_energy * 0.9,
            "denoiser should attenuate stationary noise: in={input_tail_energy} out={output_tail_energy}"
        );
    }

    #[test]
    fn aligned_frames_add_no_latency_and_keep_length() {
        let mut denoiser = Denoiser::new();
        let mut frame = noise(2 * CHUNK_SAMPLES, 0.1, 0x9e37_79b9);
        let len_before = frame.len();
        denoiser.process_frame(&mut frame);
        assert_eq!(frame.len(), len_before);
        // Chunk-aligned input is denoised immediately: nothing left queued.
        assert!(!denoiser.primed);
        assert!(denoiser.pending.is_empty());
        assert!(denoiser.processed.is_empty());
    }

    #[test]
    fn misaligned_frames_match_aligned_output_after_fixed_delay() {
        // The same signal fed in awkward 313-sample frames must produce the
        // same denoised stream as chunk-aligned feeding, shifted by exactly
        // one chunk of leading silence.
        let total = 313 * 40; // not a multiple of CHUNK_SAMPLES
        let signal = noise(total, 0.05, 0x0dd0_2211);

        let mut aligned = Denoiser::new();
        let mut aligned_out = Vec::new();
        for chunk in signal.chunks(CHUNK_SAMPLES) {
            let mut frame = chunk.to_vec();
            aligned.process_frame(&mut frame);
            if chunk.len() == CHUNK_SAMPLES {
                aligned_out.extend_from_slice(&frame);
            }
        }

        let mut misaligned = Denoiser::new();
        let mut misaligned_out = Vec::new();
        for chunk in signal.chunks(313) {
            let mut frame = chunk.to_vec();
            misaligned.process_frame(&mut frame);
            misaligned_out.extend_from_slice(&frame);
        }

        let leading = &misaligned_out[..CHUNK_SAMPLES];
        assert!(leading.iter().all(|s| *s == 0.0), "expected leading silence");
        let shifted = &misaligned_out[CHUNK_SAMPLES..];
        assert_eq!(
            &aligned_out[..shifted.len()],
            shifted,
            "misaligned feeding must reproduce the aligned stream bit-exactly"
        );
    }

    #[test]
    fn pipeline_runs_stages_in_order() {
        struct Gain(f32);
        impl EnhancementStage for Gain {
            fn process_frame(&mut self, frame: &mut [f32]) {
                for sample in frame.iter_mut() {
                    *sample *= self.0;
                }
            }
        }

        let mut pipeline = Pipeline::new()
            .with_stage(Box::new(Gain(0.5)))
            .with_stage(Box::new(Gain(0.5)));
        let mut frame = vec![1.0_f32; 8];
        pipeline.process_frame(&mut frame);
        assert!(frame.iter().all(|s| (*s - 0.25).abs() < f32::EPSILON));
    }
}
