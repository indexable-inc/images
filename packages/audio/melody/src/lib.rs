//! A deterministic pentatonic melody, written the way every shared-audio
//! instrument should be: a pure function of the absolute shared frame.
//!
//! No state survives between `sa_render` calls, so any block split, any
//! peer, and any join time produce bit-identical samples. The `render`
//! function is the whole instrument; the `sa_*` exports below are the thin
//! ABI shim the host calls through WASM.

use core::cell::UnsafeCell;

/// Controls the host writes before each render (matches the sa ABI).
pub const CONTROL_COUNT: usize = 64;
/// Largest block the host will ever request.
pub const MAX_BLOCK_FRAMES: usize = 4096;

/// controls[0]: root frequency in Hz (220 when unset).
pub const CONTROL_ROOT_HZ: usize = 0;
/// controls[1]: gain (0.2 when unset).
pub const CONTROL_GAIN: usize = 1;
/// controls[2]: melody steps per second (2 when unset).
pub const CONTROL_TEMPO: usize = 2;

/// Render `out.len()` mono frames starting at absolute shared frame
/// `start_frame`. Pure: same arguments, same bits, on every peer.
pub fn render(
    start_frame: u64,
    sample_rate: u32,
    controls: &[f32; CONTROL_COUNT],
    out: &mut [f32],
) {
    for (i, sample) in out.iter_mut().enumerate() {
        *sample = sample_at(start_frame + i as u64, sample_rate, controls);
    }
}

/// Major-pentatonic ratios over two octaves.
const RATIOS: [f64; 10] = [
    1.0,
    9.0 / 8.0,
    5.0 / 4.0,
    3.0 / 2.0,
    5.0 / 3.0,
    2.0,
    9.0 / 4.0,
    5.0 / 2.0,
    3.0,
    10.0 / 3.0,
];

fn sample_at(frame: u64, sample_rate: u32, controls: &[f32; CONTROL_COUNT]) -> f32 {
    let t = frame_seconds(frame, sample_rate);
    let root = defaulted(controls[CONTROL_ROOT_HZ], 220.0);
    let gain = defaulted(controls[CONTROL_GAIN], 0.2);
    let tempo = defaulted(controls[CONTROL_TEMPO], 2.0);

    // Which melody step we are in, and how far through it (0..1).
    let beat = t * tempo;
    let step = beat.floor();
    let pos = beat - step;

    // A deterministic pseudo-random walk over the scale: integer bit mixing
    // only, so every peer picks the identical note for the identical step.
    #[expect(
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        reason = "step is a small non-negative beat index"
    )]
    let mixed = (step as u64).wrapping_mul(0x9E37_79B9_7F4A_7C15);
    let note = RATIOS[(mixed >> 7) as usize % RATIOS.len()];

    let freq = root * note;
    let tone = 0.3f64.mul_add(triangle(2.0 * freq * t), triangle(freq * t));
    let envelope = (1.0 - pos) * (1.0 - pos);
    #[expect(
        clippy::cast_possible_truncation,
        reason = "audio samples are f32 at the ABI"
    )]
    let sample = (tone * envelope * gain) as f32;
    sample
}

fn frame_seconds(frame: u64, sample_rate: u32) -> f64 {
    #[expect(
        clippy::cast_precision_loss,
        reason = "frame counts stay far below 2^53"
    )]
    let t = frame as f64 / f64::from(sample_rate.max(1));
    t
}

/// Treat an unset (zero) control as its documented default.
fn defaulted(control: f32, default: f64) -> f64 {
    if control == 0.0 {
        default
    } else {
        f64::from(control)
    }
}

/// Triangle wave in `-1..=1` from a phase in cycles.
fn triangle(phase: f64) -> f64 {
    2.0f64.mul_add(2.0f64.mul_add(phase - phase.floor(), -1.0).abs(), -1.0)
}

// --- sa ABI v1 shim -------------------------------------------------------
// The host writes controls at `sa_controls_ptr`, calls `sa_render`, and
// reads samples at `sa_out_ptr`. Only meaningful compiled to wasm32; on
// native targets these exports are inert and `render` is used directly.

#[repr(C)]
struct Shared {
    controls: UnsafeCell<[f32; CONTROL_COUNT]>,
    out: UnsafeCell<[f32; MAX_BLOCK_FRAMES]>,
}

// SAFETY: WASM instances are single-threaded; the host serializes renders.
unsafe impl Sync for Shared {}

static SHARED: Shared = Shared {
    controls: UnsafeCell::new([0.0; CONTROL_COUNT]),
    out: UnsafeCell::new([0.0; MAX_BLOCK_FRAMES]),
};

#[expect(
    clippy::cast_possible_truncation,
    clippy::cast_possible_wrap,
    reason = "wasm32 pointers fit i32; native builds never call this"
)]
fn addr(pointer: *const f32) -> i32 {
    pointer as usize as i32
}

#[unsafe(no_mangle)]
pub const extern "C" fn sa_abi_version() -> i32 {
    1
}

#[unsafe(no_mangle)]
pub const extern "C" fn sa_channels() -> i32 {
    1
}

#[unsafe(no_mangle)]
pub extern "C" fn sa_controls_ptr() -> i32 {
    addr(SHARED.controls.get().cast())
}

#[unsafe(no_mangle)]
pub extern "C" fn sa_out_ptr() -> i32 {
    addr(SHARED.out.get().cast())
}

#[unsafe(no_mangle)]
pub extern "C" fn sa_render(start_frame: i64, frames: i32, sample_rate: i32) {
    // SAFETY: single-threaded instance; the host never renders re-entrantly.
    let controls = unsafe { &*SHARED.controls.get() };
    let out = unsafe { &mut *SHARED.out.get() };
    #[expect(
        clippy::cast_sign_loss,
        reason = "the host clamps both to non-negative"
    )]
    let (start, count) = (start_frame.max(0) as u64, sample_rate.max(1) as u32);
    #[expect(clippy::cast_sign_loss, reason = "clamped non-negative")]
    let frames = (frames.max(0) as usize).min(MAX_BLOCK_FRAMES);
    render(start, count, controls, &mut out[..frames]);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn any_block_split_is_bit_exact() {
        let controls = [0.0; CONTROL_COUNT];
        let mut whole = vec![0.0_f32; 9600];
        render(48_000, 48_000, &controls, &mut whole);

        let mut pieces = vec![0.0_f32; 9600];
        render(48_000, 48_000, &controls, &mut pieces[..1]);
        render(48_001, 48_000, &controls, &mut pieces[1..4097]);
        render(52_097, 48_000, &controls, &mut pieces[4097..]);

        let whole: Vec<u32> = whole.iter().map(|s| s.to_bits()).collect();
        let pieces: Vec<u32> = pieces.iter().map(|s| s.to_bits()).collect();
        assert_eq!(whole, pieces);
    }

    #[test]
    fn defaults_make_sound_and_controls_change_it() {
        let silent = [0.0; CONTROL_COUNT];
        let mut out = vec![0.0_f32; 4800];
        render(0, 48_000, &silent, &mut out);
        assert!(out.iter().any(|&s| s != 0.0), "defaults must be audible");

        let mut tuned = [0.0; CONTROL_COUNT];
        tuned[CONTROL_ROOT_HZ] = 440.0;
        let mut out_tuned = vec![0.0_f32; 4800];
        render(0, 48_000, &tuned, &mut out_tuned);
        assert_ne!(out, out_tuned, "root frequency control must matter");
    }
}
