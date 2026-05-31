# cuda-hello

A minimal CUDA kernel written in pure, idiomatic Rust and compiled to PTX with
[cuda-oxide](https://github.com/NVlabs/cuda-oxide), NVIDIA's experimental
Rust-to-CUDA compiler backend. Host and device code share one file
([`src/main.rs`](src/main.rs)); the kernel just writes `i*i` for each thread, the
GPU "hello, world".

This is the seed for first-class CUDA-in-Rust support in this repo. It is a draft:
the crate is written against cuda-oxide's real API, but it is not yet wired into
the Nix build (see [Status](#status)).

## Compiling vs running

Compiling CUDA needs no GPU. `rustc-codegen-cuda` turns the `#[kernel]` function
into PTX as a normal compile step, so the build runs anywhere, including CI.

Running the binary needs an NVIDIA GPU and driver, because `main` opens a CUDA
context and launches the kernel. Without a GPU you can still build and inspect the
emitted PTX; you just cannot execute it.

## Build

cuda-oxide is its own toolchain, not a library you drop into a normal build. From
this directory:

```sh
cargo oxide run        # build to PTX, then launch on the GPU
cargo oxide build      # build only (no GPU required)
```

`cargo oxide` is the driver from the cuda-oxide repo; it sets the custom codegen
backend and emits a `.ptx` next to the host binary.

### Requirements

cuda-oxide is Linux-only today (tested on Ubuntu 24.04) and pins an exact
toolchain. You need all of:

- Rust `nightly-2026-04-03` with `rust-src`, `rustc-dev`, `llvm-tools` (see
  [`rust-toolchain.toml`](rust-toolchain.toml)).
- The `cargo oxide` subcommand from cuda-oxide.
- LLVM 21+ with the NVPTX backend (`llc` on `PATH`).
- CUDA Toolkit 12.x+ and Clang/libclang headers.

The cuda-oxide dependencies are pinned by git rev in
[`Cargo.toml`](Cargo.toml); bump the rev and the toolchain channel together.

## Status

Done:

- Idiomatic single-source kernel + host launch against cuda-oxide's API.
- Pinned toolchain and dependency revs.

Not done (the work to carry this to a real `nix run .#cuda-hello`):

- Package the cuda-oxide toolchain in Nix: the nightly with `rustc-dev`, the
  `cargo oxide` driver and `librustc_codegen_cuda` backend, LLVM 21 NVPTX, and
  the CUDA toolkit, then build this crate as a nix-cargo-unit.
- A compile-only CI check that asserts the kernel still lowers to PTX (no GPU),
  so regressions in the cuda-oxide rev surface here.

See the tracking issue linked from the pull request.

---

Drafted with AI (Claude Opus 4.8) via Claude Code.
