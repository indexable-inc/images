{
  lib,
  rustPlatform,
  ix,
  cmake,
  pkg-config,
  stdenv,
}:
# External-Rust-tool house style: a standalone third-party binary built from a
# pinned flake source input with `rustPlatform.buildRustPackage`. See
# `skills/dependency-intake/SKILL.md` and `packages/launchk`.
#
# fff is a fast file-search toolkit for humans and AI agents. Two artifacts ship
# from one build:
#   * `bin/fff-mcp`        – the CLI / MCP server over the in-memory file index.
#   * `lib/libfff_c.{so,dylib}` – the stable C ABI (crate `fff-c`), which the
#     `mcp` package loads via ctypes to expose `import fff` in notebook sessions.
let
  src = ix.fffSrc;
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

  # Build the fff-mcp binary and the fff-c cdylib in one cargo invocation so
  # both artifacts share the dependency compile. Scoping to these two packages
  # (rather than the whole workspace) keeps out fff-nvim's mlua/Lua build.
  # `--no-default-features` skips the workspace's default `zlob` feature, which
  # shells out to a system Zig install at build time (see
  # crates/fff-core/build.rs); without it the crate falls back to the pure-Rust
  # globset matcher.
  cargoBuildFlags = [
    "--package"
    "fff-mcp"
    "--package"
    "fff-c"
  ];
  cargoTestFlags = [
    "--package"
    "fff-mcp"
    "--package"
    "fff-c"
  ];
  buildNoDefaultFeatures = true;

  # buildRustPackage installs the binary to bin/, but the fff-c cdylib is not
  # picked up automatically. Copy the unhashed final artifact into lib/ (skip
  # the hashed copy under deps/) for both the Linux .so and the macOS .dylib.
  postInstall = ''
    # shell
    mkdir -p "$out/lib"
    find target -type f \( -name 'libfff_c.so' -o -name 'libfff_c.dylib' \) \
      -not -path '*/deps/*' -exec install -Dm555 {} "$out/lib/" \;
    if [ -z "$(ls -A "$out/lib" 2>/dev/null)" ]; then
      echo "fff: no libfff_c cdylib found under target/" >&2
      find target -name 'libfff_c.*' >&2 || true
      exit 1
    fi
  '';

  meta = {
    description = "Fast file-search toolkit for humans and AI agents (fff-mcp CLI / MCP server + fff-c cdylib)";
    homepage = "https://github.com/dmtrKovalenko/fff";
    license = lib.licenses.mit;
    mainProgram = "fff-mcp";
    platforms = lib.platforms.unix;
  };
}
