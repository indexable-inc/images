{
  lib,
  rustPlatform,
  ix,
}:
# Reference package for the external-Rust-tool house style: a standalone
# third-party binary built from a pinned flake source input with
# `rustPlatform.buildRustPackage`. See `skills/dependency-intake/SKILL.md`.
let
  src = ix.launchkSrc;
in
  rustPlatform.buildRustPackage {
    pname = "launchk";
    # No upstream release tag past the crate version; pin to the master rev with
    # the nixpkgs unstable-version spelling so a bump reads as a dated change.
    version = "0.3.1-unstable-2025-06-07";

    inherit src;

    # launchk commits a pure-crates.io Cargo.lock, so read it straight from the
    # source: a rev bump carries the dependency set with no checked-in lock to
    # drift and no coarse cargoHash to refresh by hand.
    cargoLock.lockFile = src + "/Cargo.lock";

    strictDeps = true;

    # xpc-sys generates the XPC framework bindings with bindgen, which needs
    # libclang on the build host.
    nativeBuildInputs = [rustPlatform.bindgenHook];

    # `git_version!()` shells out to `git describe` at build time; the fetched
    # tarball has no `.git`, so resolve the about-box string to the crate version
    # instead. --replace-fail keeps this guard honest if upstream moves the call.
    postPatch = ''
      # shell
      substituteInPlace launchk/src/main.rs \
        --replace-fail "git_version!()" 'env!("CARGO_PKG_VERSION")'
    '';

    cargoBuildFlags = [
      "-p"
      "launchk"
    ];
    cargoTestFlags = [
      "-p"
      "launchk"
    ];

    meta = {
      description = "Cursive TUI for observing launchd agents and daemons";
      homepage = "https://github.com/mach-kernel/launchk";
      license = lib.licenses.mit;
      mainProgram = "launchk";
      platforms = lib.platforms.darwin;
    };
  }
