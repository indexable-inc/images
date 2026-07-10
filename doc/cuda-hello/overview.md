# cuda-hello

`packages/cuda-hello` is a minimal CUDA kernel written in pure, idiomatic Rust
and compiled to PTX with [cuda-oxide](https://github.com/NVlabs/cuda-oxide),
NVIDIA's experimental Rust-to-CUDA codegen backend. Host and device code share
one file (`src/main.rs`); each GPU thread writes the square of its own index, the
GPU "hello, world" (`README.md:1-12`). It is the seed for first-class
CUDA-in-Rust support in the repo, written against cuda-oxide's real API and
verified to lower to PTX.

## Why this crate is standalone

Unlike every other package in this domain, `cuda-hello` is deliberately not a
member of the index cargo workspace and has no `package.nix`, so there is no
`nix run .#cuda-hello` and the root `nix flake check` never builds it
(`Cargo.toml:1-15`, `README.md:53-59`). The detachment is structural:

- An empty `[workspace]` table in `Cargo.toml` makes the crate its own workspace
  root, off the repo's workspace toolchain (`Cargo.toml:15`).
- cuda-oxide is a custom rustc codegen backend that hooks unstable rustc
  internals, so it needs its own pinned nightly, the `cargo oxide` driver, LLVM
  22 with the NVPTX backend, and the CUDA toolkit. The index workspace pins a
  different stable toolchain that cannot build it (`Cargo.toml:1-8`).
- A crate-local `flake.nix` carries that toolchain instead, exposing only dev
  shells (no packages), so the repo flake is untouched.

## Toolchain and pinning

- `rust-toolchain.toml` pins `nightly-2026-04-03` with `rust-src`, `rustc-dev`,
  `rust-analyzer`, `clippy`, and `llvm-tools`; the `rustc-dev`/`rust-src`/
  `llvm-tools` components are required because cuda-oxide hooks rustc internals
  (`rust-toolchain.toml:1-6`).
- The cuda-oxide rev `d22af5f29738fce099ae38262faa7ab59828865f` is pinned in
  lockstep in both `Cargo.toml` (the `cuda-device`, `cuda-host`, `cuda-core` git
  deps) and `flake.nix` (the `cuda-oxide` input, with `nixpkgs` following it);
  bump the rev and the toolchain channel together (`Cargo.toml:17-24`,
  `flake.nix:4-11`).
- `flake.nix` provides `devShells.default` for `x86_64-linux` and `aarch64-linux`
  (cuda-oxide is Linux-only), inheriting cuda-oxide's dev shell verbatim via
  `inputsFrom` so the full CUDA + Rust environment (driver, nightly, LLVM 22 with
  NVPTX, CUDA 13, libclang) needs no manual setup (`flake.nix:13-40`).

## Dependencies (`Cargo.toml:17-31`)

- `cuda-device` - device-side intrinsics: thread indexing, the `#[kernel]` /
  `#[cuda_module]` attributes, and the bounds-checked `DisjointSlice` device
  pointer.
- `cuda-core` - host-side CUDA runtime: context, stream, device buffers, launch
  config.
- `cuda-host` - host-side launch glue used only by the code the `#[cuda_module]` /
  `#[kernel]` macros generate; `cargo-machete` is told to ignore it
  (`Cargo.toml:26-31`) since its source scan cannot see macro-generated uses.

## Kernel and host (`src/main.rs`)

A `#[cuda_module] mod kernels` holds one `#[kernel] fn squares(mut out:
DisjointSlice<u32>)`: each thread reads its 1-D global index via
`thread::index_1d()` and, if `out.get_mut(idx)` is in bounds, writes `i * i`.
`get_mut` returns `None` past the buffer end, so an over-launched grid can never
write out of bounds (`src/main.rs:18-40`). `THREADS = 256` (`src/main.rs:16`).

`main` is ordinary host Rust routed to the normal backend: open
`CudaContext::new(0)`, take the default stream, allocate a zeroed
`DeviceBuffer<u32>`, `kernels::load(&ctx)` the embedded PTX module, launch
`module.squares(...)` with `LaunchConfig::for_num_elems(THREADS)`, copy results
back with `to_host_vec`, and assert each element equals `i * i`
(`src/main.rs:42-70`). `rustc-codegen-cuda` routes the `#[kernel]` function
through the Rust -> MIR -> PTX pipeline and leaves `main` to the host backend, so
one `cargo oxide run` produces a host binary with the PTX embedded.

## Building and running (`README.md:14-52`)

Compiling needs no GPU: lowering the kernel to PTX is a normal compile step that
runs anywhere, including CI. Running the binary needs an NVIDIA GPU and driver,
because `main` opens a CUDA context and launches the kernel.

```sh
nix develop            # enter the cuda-oxide toolchain (Linux only)
cargo oxide build      # compile to PTX (no GPU required)
cargo oxide run        # compile to PTX, then launch on the GPU
```

`cargo oxide` is cuda-oxide's driver: it sets the custom codegen backend and
emits a `.ptx` next to the host binary. On a standalone crate it auto-fetches and
builds `librustc_codegen_cuda.so` on first use and caches it (outside the Nix
store), so the first build is much slower than later ones.

## Status

Verified end to end on the compile path (the kernel lowers to PTX via `cargo
oxide build`, no GPU); the run path needs an NVIDIA device. Not yet done, and the
work to reach a pure `nix build .#cuda-hello` (`README.md:61-79`):

- Package the cuda-oxide backend in a pure derivation and build this crate as a
  nix-cargo-unit. cuda-oxide itself does not yet ship a pure build
  (`librustc_codegen_cuda.so` is built on first use and cached outside the Nix
  store), so this is upstream work as much as repo work.
- A compile-only CI check that asserts the kernel still lowers to PTX, so a bad
  cuda-oxide rev bump surfaces here.
