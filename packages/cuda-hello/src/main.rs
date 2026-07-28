//! The smallest interesting cuda-oxide program: one kernel, written in plain
//! Rust, compiled straight to PTX and launched on the GPU.
//!
//! Host code and device code live in this one file. `rustc-codegen-cuda` routes
//! the `#[kernel]` function through the Rust -> MIR -> PTX pipeline and leaves
//! `main` to the normal host backend, so a single `cargo oxide run` produces a
//! host binary with the PTX embedded.
//!
//! Compiling needs no GPU (it is pure CPU work, fine for CI). Running `main`
//! needs an NVIDIA GPU and driver, because that is where the kernel executes.

use cuda_core::{CudaContext, DeviceBuffer, LaunchConfig};
use cuda_device::{DisjointSlice, cuda_module, kernel, thread};

/// How many threads to launch, one per output element.
const THREADS: usize = 256;

#[cuda_module]
mod kernels {
    use super::*;

    /// Each thread writes the square of its own global index.
    ///
    /// No inputs: the work is derived entirely from the thread index, which
    /// makes this the GPU equivalent of "hello, world" while still doing real
    /// arithmetic and a bounds-checked device write.
    #[kernel]
    pub fn squares(mut out: DisjointSlice<u32>) {
        let idx = thread::index_1d();
        // Read the raw index before `get_mut` consumes the (non-`Copy`)
        // `ThreadIndex`. It stays well under `u32::MAX` for any realistic
        // launch, so the square below cannot overflow.
        let i = idx.get() as u32;
        // `get_mut` returns `None` for threads past the buffer end, so an
        // over-launched grid can never write out of bounds.
        if let Some(slot) = out.get_mut(idx) {
            *slot = i * i;
        }
    }
}

fn main() {
    let ctx = CudaContext::new(0).expect("open CUDA device 0");
    let stream = ctx.default_stream();

    let mut out = DeviceBuffer::<u32>::zeroed(&stream, THREADS).expect("allocate output buffer");

    let module = kernels::load(&ctx).expect("load embedded PTX module");
    module
        .squares(
            &stream,
            LaunchConfig::for_num_elems(THREADS as u32),
            &mut out,
        )
        .expect("launch squares kernel");

    let host = out.to_host_vec(&stream).expect("copy results back to host");

    for i in 0..5 {
        println!("thread {i}: {i}^2 = {}", host[i]);
    }

    let correct = host
        .iter()
        .enumerate()
        .all(|(i, &value)| value == (i as u32) * (i as u32));
    assert!(correct, "GPU result did not match i*i");

    println!("hello from {THREADS} GPU threads");
}
