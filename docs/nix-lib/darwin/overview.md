# lib/darwin: macOS cross-compilation helpers

`lib/darwin/` cross-compiles a Rust workspace (and its C/C++ deps) from Linux to
Darwin without an Apple machine. Two files: a pinned macOS SDK and the zig + SDK
toolchain that consumes it. `lib/default.nix` exposes both as `ix.macosSdk` and
`ix.appleSdkToolchain` (`lib/default.nix:368-380`); the Rust workspace's cross
path wires them together (`lib/rust/workspace.nix:147-157`,
`unitsFor { target }`).

## macos-sdk.nix

`macosSdk { pkgs }` (`lib/darwin/macos-sdk.nix:10`) fetches a public repackaged
`MacOSX15.4.sdk` tarball pinned by SRI hash and unpacks it
(`lib/darwin/macos-sdk.nix:12-20`). Clang and zig respect `-isysroot` / `SDKROOT`
pointing at the unpacked SDK, so one fetched SDK covers both the C compile and
the Rust link. Apple licenses the SDK for use on Apple hardware; override
`ix.macosSdk` with your own SDK derivation for a stricter licensing posture
(`lib/darwin/macos-sdk.nix:6-9`). The same tarball + hash is shared with the
sibling `ix` repo, so the store path is shared.

## apple-sdk-toolchain.nix

`appleSdkToolchain { appleSdk, lib, pkgs, target }`
(`lib/darwin/apple-sdk-toolchain.nix:13-18`) builds a cross toolchain for a
Darwin `target` (`aarch64-apple-darwin` or `x86_64-apple-darwin`; others throw,
`lib/darwin/apple-sdk-toolchain.nix:232-233`). `zig cc` / `zig c++` are the cross
C/C++ compilers with the SDK as sysroot; `clang -fuse-ld=lld` is the Rust linker
(`lib/darwin/apple-sdk-toolchain.nix:1-7`). The wrappers go through the checked
`writeBashApplication` (so `bash -n` + shellcheck run at build time) and rewrite
incoming args: normalize Apple `--target=` spellings to zig's, drop `-arch`/`-m64`
and sanitizer flags, and pin `ZIG_GLOBAL_CACHE_DIR` to an action-local writable
path so a sandboxed `HOME=/var/empty` does not make zig fail silently
(`lib/darwin/apple-sdk-toolchain.nix:30-138`).

It returns three fields the Rust workspace consumes
(`lib/darwin/apple-sdk-toolchain.nix:234-275`):

- `env`: `CC`/`CXX`/`AR`/`RANLIB`/`SDKROOT`/`CFLAGS`/`CMAKE_TOOLCHAIN_FILE` plus
  the per-target `CARGO_TARGET_<T>_LINKER` and `CARGO_TARGET_<T>_AR` the
  nix-cargo-unit renderer picks up per unit.
- `runtimeInputs`: the wrapper packages (cc/cxx/linker/ar/ranlib/xcrun) + LLVM
  bintools.
- `rustcArgsForPlatform platform`: returns the framework search path only when
  `platform == target` (`lib/darwin/apple-sdk-toolchain.nix:239-244`), so host
  (platform=null) and other-target units are unaffected. This is the hook
  `buildWorkspace` calls per unit.

`lib/rust/workspace.nix` invokes `appleSdkToolchain` for an
`*-apple-darwin` cross target and folds its `env`/`runtimeInputs`/
`extraRustcArgsForPlatform` into the cross unit graph
(`lib/rust/workspace.nix:147-157`, `236`, `281-282`). See
[rust](../rust/overview.md) for the cross-graph entry point `unitsFor`.
