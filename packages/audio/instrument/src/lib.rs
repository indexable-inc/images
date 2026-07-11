//! Deterministic WASM instrument host.
//!
//! An instrument is a sandboxed pure function `(controls, time) -> samples`
//! compiled to WASM (from `FunDSP`, hand-written WAT, or anything else). Every
//! peer runs the same module bytes over the same shared timeline, so audio
//! is identical everywhere without ever sending it.
//!
//! # ABI v1
//!
//! A module exports:
//! - `memory`: the linear memory the pointers below index into.
//! - `sa_abi_version() -> i32`: must return [`ABI_VERSION`].
//! - `sa_channels() -> i32`: output channel count, `1..=2`.
//! - `sa_controls_ptr() -> i32`: address of `f32[CONTROL_COUNT]` the host
//!   writes control values into before each render.
//! - `sa_out_ptr() -> i32`: address of `f32[MAX_BLOCK_FRAMES * channels]`
//!   (interleaved) the module renders into.
//! - `sa_render(start_frame: i64, nframes: i32, sample_rate: i32)`: fill the
//!   output buffer for shared-timeline frames `start_frame..+nframes`.
//!
//! Time arrives as the *absolute* shared frame number, so a stateless module
//! (phase computed from `start_frame`) is bit-exact regardless of when a
//! peer joined or how the host splits blocks.
//!
//! # Determinism
//!
//! The engine [`wasmtime::Config`] canonicalizes NaNs and disables relaxed
//! SIMD, the two WASM escape hatches from bit-exact float semantics, so
//! identical `(module, controls, frames)` yields identical samples on every
//! host and architecture.

use anyhow::{Context as _, Result, ensure};
use wasmtime::{Engine, Instance, Memory, Module, Store, TypedFunc};

/// The ABI revision this host speaks.
pub const ABI_VERSION: i32 = 1;
/// Number of `f32` control slots the host writes before each render.
pub const CONTROL_COUNT: usize = 64;
/// Upper bound on frames per `sa_render` call; callers split larger spans.
pub const MAX_BLOCK_FRAMES: usize = 4096;

/// A loaded, validated instrument module, ready to render.
pub struct Instrument {
    store: Store<()>,
    memory: Memory,
    render: TypedFunc<(i64, i32, i32), ()>,
    controls_ptr: usize,
    out_ptr: usize,
    channels: u32,
    /// Scratch byte buffers so [`render`](Self::render) never allocates.
    controls_bytes: Vec<u8>,
    out_bytes: Vec<u8>,
}

impl std::fmt::Debug for Instrument {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Instrument")
            .field("channels", &self.channels)
            .finish_non_exhaustive()
    }
}

impl Instrument {
    /// Compile and instantiate a module from `.wasm` or `.wat` bytes and
    /// validate its ABI. Compilation is the expensive step; keep the
    /// instance and call [`render`](Self::render) per block.
    ///
    /// # Errors
    /// Fails when the bytes are not a valid module, a required export is
    /// missing or mistyped, the ABI version differs, or the declared
    /// buffers fall outside linear memory.
    pub fn load(bytes: &[u8]) -> Result<Self> {
        let engine = Engine::new(&deterministic_config())?;
        let module = Module::new(&engine, bytes)
            .map_err(anyhow::Error::from)
            .context("compile instrument module")?;
        let mut store = Store::new(&engine, ());
        let instance = Instance::new(&mut store, &module, &[])
            .map_err(anyhow::Error::from)
            .context("instantiate instrument module")?;

        let abi: TypedFunc<(), i32> = instance.get_typed_func(&mut store, "sa_abi_version")?;
        let abi = abi.call(&mut store, ())?;
        ensure!(
            abi == ABI_VERSION,
            "instrument speaks ABI v{abi}, host speaks v{ABI_VERSION}"
        );

        let channels: TypedFunc<(), i32> = instance.get_typed_func(&mut store, "sa_channels")?;
        let channels = u32::try_from(channels.call(&mut store, ())?)
            .ok()
            .filter(|&channels| (1..=2).contains(&channels))
            .context("sa_channels must be 1 or 2")?;

        let memory = instance
            .get_memory(&mut store, "memory")
            .context("instrument exports no `memory`")?;
        let controls_ptr = read_ptr(&instance, &mut store, "sa_controls_ptr")?;
        let out_ptr = read_ptr(&instance, &mut store, "sa_out_ptr")?;
        let render = instance.get_typed_func(&mut store, "sa_render")?;

        let instrument = Self {
            store,
            memory,
            render,
            controls_ptr,
            out_ptr,
            channels,
            controls_bytes: vec![0; CONTROL_COUNT * 4],
            out_bytes: vec![0; MAX_BLOCK_FRAMES * channels as usize * 4],
        };
        instrument.check_bounds()?;
        Ok(instrument)
    }

    /// Output channel count (1 = mono, 2 = interleaved stereo).
    #[must_use]
    pub const fn channels(&self) -> u32 {
        self.channels
    }

    /// Render `frames` frames starting at absolute shared-timeline frame
    /// `start_frame` into `out` (interleaved, `frames * channels` samples),
    /// with `controls` visible to the module. Allocation-free after load.
    ///
    /// # Errors
    /// Fails when `frames` exceeds [`MAX_BLOCK_FRAMES`], `out` is missized,
    /// or the module traps.
    ///
    /// # Panics
    /// Never: `frames` is checked against [`MAX_BLOCK_FRAMES`] before the
    /// `i32` conversion.
    pub fn render(
        &mut self,
        start_frame: u64,
        frames: usize,
        sample_rate: u32,
        controls: &[f32; CONTROL_COUNT],
        out: &mut [f32],
    ) -> Result<()> {
        ensure!(
            frames <= MAX_BLOCK_FRAMES,
            "block of {frames} frames exceeds {MAX_BLOCK_FRAMES}"
        );
        let samples = frames * self.channels as usize;
        ensure!(
            out.len() == samples,
            "out has {} samples, block needs {samples}",
            out.len()
        );

        // WASM linear memory is little-endian by spec; `to_le_bytes` writes
        // what the module expects even on a big-endian host.
        for (slot, value) in self.controls_bytes.chunks_exact_mut(4).zip(controls) {
            slot.copy_from_slice(&value.to_le_bytes());
        }
        self.memory
            .write(&mut self.store, self.controls_ptr, &self.controls_bytes)?;

        let start = i64::try_from(start_frame).context("frame exceeds i64::MAX")?;
        let frames_i32 = i32::try_from(frames).expect("frames <= MAX_BLOCK_FRAMES fits i32");
        let sample_rate = i32::try_from(sample_rate).context("sample rate exceeds i32::MAX")?;
        self.render
            .call(&mut self.store, (start, frames_i32, sample_rate))
            .map_err(anyhow::Error::from)
            .context("instrument render trapped")?;

        let bytes = &mut self.out_bytes[..samples * 4];
        self.memory.read(&self.store, self.out_ptr, bytes)?;
        for (sample, chunk) in out.iter_mut().zip(bytes.chunks_exact(4)) {
            *sample = f32::from_le_bytes(chunk.try_into().expect("chunks_exact(4)"));
        }
        Ok(())
    }

    /// Reject modules whose declared buffers escape linear memory, once at
    /// load time, so renders never do bounds arithmetic.
    fn check_bounds(&self) -> Result<()> {
        let size = self.memory.data_size(&self.store);
        let controls_end = self.controls_ptr + CONTROL_COUNT * 4;
        let out_end = self.out_ptr + MAX_BLOCK_FRAMES * self.channels as usize * 4;
        ensure!(
            controls_end <= size && out_end <= size,
            "instrument buffers exceed its {size}-byte memory"
        );
        Ok(())
    }
}

/// An engine configuration pinned to bit-exact semantics.
fn deterministic_config() -> wasmtime::Config {
    let mut config = wasmtime::Config::new();
    config.cranelift_nan_canonicalization(true);
    config.wasm_relaxed_simd(false);
    config
}

fn read_ptr(instance: &Instance, store: &mut Store<()>, name: &str) -> Result<usize> {
    let func: TypedFunc<(), i32> = instance.get_typed_func(&mut *store, name)?;
    let ptr = func.call(&mut *store, ())?;
    usize::try_from(ptr).with_context(|| format!("{name} returned negative pointer {ptr}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A minimal ABI v1 instrument: mono, sample = fract(frame / 1000) * gain
    /// where gain = controls[1]. Stateless (pure function of the absolute
    /// frame), so it is bit-exact under any block split.
    const RAMP_WAT: &str = r#"
(module
  (memory (export "memory") 2)
  (func (export "sa_abi_version") (result i32) (i32.const 1))
  (func (export "sa_channels") (result i32) (i32.const 1))
  (func (export "sa_controls_ptr") (result i32) (i32.const 0))
  (func (export "sa_out_ptr") (result i32) (i32.const 256))
  (func (export "sa_render") (param $start i64) (param $n i32) (param $sr i32)
    (local $i i32)
    (local $frame i64)
    (local $gain f32)
    (local.set $gain (f32.load (i32.const 4)))
    (block $done
      (loop $loop
        (br_if $done (i32.ge_s (local.get $i) (local.get $n)))
        (local.set $frame (i64.add (local.get $start) (i64.extend_i32_s (local.get $i))))
        (f32.store
          (i32.add (i32.const 256) (i32.mul (local.get $i) (i32.const 4)))
          (f32.mul
            (f32.div
              (f32.convert_i64_s (i64.rem_s (local.get $frame) (i64.const 1000)))
              (f32.const 1000))
            (local.get $gain)))
        (local.set $i (i32.add (local.get $i) (i32.const 1)))
        (br $loop))))
)
"#;

    #[test]
    fn loads_and_renders_a_wat_instrument() -> Result<()> {
        let mut instrument = Instrument::load(RAMP_WAT.as_bytes())?;
        assert_eq!(instrument.channels(), 1);
        let mut controls = [0.0; CONTROL_COUNT];
        controls[1] = 2.0;
        let mut out = vec![0.0; 8];
        instrument.render(500, 8, 48_000, &controls, &mut out)?;
        assert!((out[0] - 1.0).abs() < 1e-6, "frame 500 -> 0.5 * gain 2.0");
        assert!((out[1] - 1.002).abs() < 1e-6);
        Ok(())
    }

    #[test]
    fn block_split_is_bit_exact() -> Result<()> {
        let mut a = Instrument::load(RAMP_WAT.as_bytes())?;
        let mut b = Instrument::load(RAMP_WAT.as_bytes())?;
        let mut controls = [0.0; CONTROL_COUNT];
        controls[1] = 0.7;

        let mut whole = vec![0.0; 1024];
        a.render(10_000, 1024, 48_000, &controls, &mut whole)?;

        let mut split = vec![0.0; 1024];
        b.render(10_000, 100, 48_000, &controls, &mut split[..100])?;
        b.render(10_100, 924, 48_000, &controls, &mut split[100..])?;

        let whole: Vec<u32> = whole.iter().map(|sample| sample.to_bits()).collect();
        let split: Vec<u32> = split.iter().map(|sample| sample.to_bits()).collect();
        assert_eq!(whole, split);
        Ok(())
    }

    /// A do-nothing module with parameterized ABI-probe exports, for
    /// exercising `Instrument::load` validation failures.
    fn stub_wat(abi_version: i32, memory_pages: u32, out_ptr: u32) -> String {
        format!(
            r#"
(module
  (memory (export "memory") {memory_pages})
  (func (export "sa_abi_version") (result i32) (i32.const {abi_version}))
  (func (export "sa_channels") (result i32) (i32.const 1))
  (func (export "sa_controls_ptr") (result i32) (i32.const 0))
  (func (export "sa_out_ptr") (result i32) (i32.const {out_ptr}))
  (func (export "sa_render") (param i64) (param i32) (param i32)))
"#
        )
    }

    #[test]
    fn rejects_wrong_abi_version() {
        let error = Instrument::load(stub_wat(2, 2, 256).as_bytes()).expect_err("ABI v2 rejected");
        assert!(error.to_string().contains("ABI"));
    }

    #[test]
    fn rejects_buffers_outside_memory() {
        // One 64 KiB page cannot hold an out buffer at page end.
        let error = Instrument::load(stub_wat(1, 1, 65_000).as_bytes())
            .expect_err("oversized buffers rejected");
        assert!(error.to_string().contains("memory"));
    }

    #[test]
    fn rejects_missing_export() {
        let wat = r#"(module (memory (export "memory") 1))"#;
        assert!(Instrument::load(wat.as_bytes()).is_err());
    }
}
