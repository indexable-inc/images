{
  ix,
  lib,
  makeWrapper,
  clippy-src ? null,
}:
# The upstream source comes in as `clippy-src`, never a `src` argument: a `src`
# formal collides with `pkgs.src`, which `callPackage` auto-binds over the
# default. nixpkgs renamed that package to a throw (2025-11-19), so an auto-
# bound `src` turned the discovered `packages.<system>.llm-clippy` output into
# an eval error that `nix flake check` surfaces.
let
  # `ix.buildRustPackage` is curried on the full package set; read `pkgs` from
  # `ix` rather than taking a `pkgs` callPackage formal (unreachable by `override`).
  inherit (ix) pkgs;
  # De-forking: `clippy-src` is now upstream rust-lang/rust-clippy pinned by rev,
  # and the LLM-tuned lint series lives in ./patches (applied via the shared
  # patched-src util). The patched tree carries the toolchain file and Cargo.lock
  # the build reads below, exactly as the old fork tree did.
  source =
    if clippy-src == null
    then throw "llm-clippy: clippy-src is required"
    else
      ix.patchedSrc {
        name = "llm-clippy";
        src = clippy-src;
        patchDir = ./patches;
      };
  # Drive the toolchain from the patched tree's `rust-toolchain.toml` so the
  # base bump + regenerated series advance the rustc/rustc_private ABI in
  # lockstep. clippy is nightly-coupled: `clippy-src` is pinned by rev and must
  # move with the pinned nightly, never free-float (see flake.nix).
  toolchain = pkgs.rust-bin.fromRustupToolchainFile (source + "/rust-toolchain.toml");

  rustcLibPathVar =
    if pkgs.stdenv.hostPlatform.isDarwin
    then "DYLD_LIBRARY_PATH"
    else "LD_LIBRARY_PATH";
in
  ix.buildRustPackage pkgs {
    pname = "llm-clippy";
    version = "0.1.97";

    src = source;
    rustToolchain = toolchain;
    # Read the lockfile straight from the patched tree (patch 0014 tracks
    # Cargo.lock) so a base bump + regenerated series brings dependency changes
    # along with the source. No separately checked-in lockfile to drift.
    cargoLock.lockFile = source + "/Cargo.lock";

    nativeBuildInputs = [makeWrapper];
    buildInputs =
      [
        pkgs.zlib
      ]
      ++ lib.optional pkgs.stdenv.hostPlatform.isDarwin pkgs.libiconv;
    doCheck = false;
    # llm-clippy is the Clippy binary that cargo-unit policy checks use for the
    # rest of the repo. Its own build stays on the bootstrap path, where a Clippy
    # policy check would recurse through `llmClippyFor`.
    policy.clippy.enable = false;
    # The Clippy fork carries Cargo UI fixtures that are not build workspaces.
    # cargo-machete walks them anyway and cargo metadata tries to write lockfiles
    # inside the read-only Nix store.
    policy.cargoMachete.enable = false;

    # This Clippy fork links against rustc_private crates from its Rust toolchain.
    env.RUSTC_BOOTSTRAP = "1";

    postInstall = ''
      # shell
      for bin in "$out/bin/cargo-clippy" "$out/bin/clippy-driver"; do
        wrapProgram "$bin" \
          --prefix ${rustcLibPathVar} : "${toolchain}/lib"
      done
    '';

    meta = {
      description = "Clippy tuned for LLM-assisted codebases";
      homepage = "https://github.com/rust-lang/rust-clippy";
      license = [
        lib.licenses.asl20
        lib.licenses.mit
      ];
      mainProgram = "clippy-driver";
    };

    passthru = {
      inherit toolchain;
    };
  }
