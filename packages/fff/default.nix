{
  lib,
  rustPlatform,
  fetchFromGitHub,
  cmake,
  pkg-config,
  stdenv,
}:
# External-Rust-tool house style: a standalone third-party binary built from a
# pinned `fetchFromGitHub` rev with `rustPlatform.buildRustPackage`. See
# `agent-context/sections/13-dependency-intake.md` and `packages/launchk`.
#
# fff is a fast file-search toolkit for humans and AI agents. The shipped binary
# is `fff-mcp`: a CLI / MCP server over the in-memory file + content index.
let
  src = fetchFromGitHub {
    owner = "dmtrKovalenko";
    repo = "fff";
    rev = "v0.9.1";
    hash = "sha256-6ZmEeN/Ued9FZo/qfUb8/0L02F+8ECV0smAiQvIqyzU=";
  };
in
rustPlatform.buildRustPackage {
  pname = "fff";
  version = "0.9.1";

  inherit src;

  # fff commits a pure-crates.io Cargo.lock, so read it straight from the
  # source: a rev bump carries the dependency set with no coarse cargoHash to
  # refresh by hand.
  cargoLock.lockFile = src + "/Cargo.lock";

  strictDeps = true;

  # libgit2-sys (git2 with vendored-libgit2) and lmdb-master-sys (heed) build
  # their C deps; pkg-config and cmake are the build-host tools they probe for.
  nativeBuildInputs = [
    cmake
    pkg-config
  ];

  # Build only the fff-mcp binary and skip the workspace's default `zlob`
  # feature, which shells out to a system Zig install at build time (see
  # crates/fff-core/build.rs). Without zlob the crate falls back to the pure-Rust
  # globset matcher.
  buildAndTestSubdir = "crates/fff-mcp";
  buildNoDefaultFeatures = true;

  meta = {
    description = "Fast file-search toolkit for humans and AI agents (fff-mcp CLI / MCP server)";
    homepage = "https://github.com/dmtrKovalenko/fff";
    license = lib.licenses.mit;
    mainProgram = "fff-mcp";
    platforms = lib.platforms.unix;
  };
}
