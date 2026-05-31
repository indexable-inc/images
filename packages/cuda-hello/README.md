# cuda-hello

A minimal CUDA kernel written in pure, idiomatic Rust and compiled to PTX with
[cuda-oxide](https://github.com/NVlabs/cuda-oxide), NVIDIA's experimental
Rust-to-CUDA compiler backend. Host and device code share one file
([`src/main.rs`](src/main.rs)); the kernel writes `i*i` for each thread, the GPU
"hello, world".

This is the seed for first-class CUDA-in-Rust support in this repo. The kernel is
written against cuda-oxide's real API (a faithful subset of upstream's `vecadd`
example) and is verified to lower to PTX; the crate is deliberately standalone
(see [Why standalone](#why-standalone)).

## Compiling vs running

Compiling CUDA needs no GPU. `rustc-codegen-cuda` turns the `#[kernel]` function
into PTX as a normal compile step, so the build runs anywhere, including CI.

Running the binary needs an NVIDIA GPU and driver, because `main` opens a CUDA
context and launches the kernel. Without a GPU you can still build and inspect the
emitted PTX; you just cannot execute it.

## Build

The crate-local [`flake.nix`](flake.nix) inherits cuda-oxide's dev shell, so you
get the exact toolchain (the `cargo oxide` driver, the pinned nightly, LLVM 22
with NVPTX, CUDA 13, and libclang) with no manual setup. From this directory:

```sh
nix develop            # enter the cuda-oxide toolchain (Linux only)
cargo oxide build      # compile to PTX (no GPU required)
cargo oxide run        # compile to PTX, then launch on the GPU
```

`cargo oxide` is cuda-oxide's driver: it sets the custom codegen backend and emits
a `.ptx` next to the host binary. On a standalone crate like this one it
auto-fetches and builds `librustc_codegen_cuda.so` on first use and caches it, so
the first build is much slower than later ones.

### Without Nix

cuda-oxide is Linux-only today. Outside the dev shell you need all of:

- Rust `nightly-2026-04-03` with `rust-src`, `rustc-dev`, `llvm-tools` (see
  [`rust-toolchain.toml`](rust-toolchain.toml)).
- The `cargo oxide` subcommand from cuda-oxide.
- LLVM 22 with the NVPTX backend (`llc` on `PATH`).
- CUDA Toolkit 13.x and Clang/libclang headers.

The cuda-oxide rev is pinned in lockstep in both [`Cargo.toml`](Cargo.toml) and
[`flake.nix`](flake.nix); bump the rev and the toolchain channel together.

## Why standalone

The crate is intentionally not a member of the index cargo workspace (note the
empty `[workspace]` table in `Cargo.toml`) and has no `package.nix`, so the repo's
stable toolchain never tries to build it and the root `nix flake check` is
untouched. cuda-oxide is its own toolchain on a pinned nightly that the workspace
toolchain cannot build; the crate-local flake keeps that toolchain self-contained.

## Status

Done:

- Idiomatic single-source kernel + host launch against cuda-oxide's real API.
- Toolchain and dependency revs pinned in lockstep, provided by `nix develop`.
- Verified end to end on the compile path: the kernel lowers to PTX via
  `cargo oxide build` (no GPU). The run path needs an NVIDIA device.

Not done (the work to carry this to a pure `nix build .#cuda-hello`):

- Package the cuda-oxide backend in a pure derivation and build this crate as a
  nix-cargo-unit. cuda-oxide itself does not yet ship a pure build
  (`librustc_codegen_cuda.so` is built on first use and cached outside the Nix
  store), so this is upstream work as much as repo work.
- A compile-only CI check that asserts the kernel still lowers to PTX, so a bad
  cuda-oxide rev bump surfaces here.

See the tracking issue linked from the pull request.

---

Drafted with AI (Claude Opus 4.8) via Claude Code.
