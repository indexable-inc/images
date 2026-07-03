{
  ix,
  lib,
  makeWrapper,
  clippy-fork ? null,
}:
# The fork source comes in as `clippy-fork`, never a `src` argument: a `src`
# formal collides with `pkgs.src`, which `callPackage` auto-binds over the
# default. nixpkgs renamed that package to a throw (2025-11-19), so an auto-
# bound `src` turned the discovered `packages.<system>.llm-clippy` output into
# an eval error that `nix flake check` surfaces.
let
  # `ix.buildRustPackage` is curried on the full package set; read `pkgs` from
  # `ix` rather than taking a `pkgs` callPackage formal (unreachable by `override`).
  inherit (ix) pkgs;
  source =
    if clippy-fork == null
    then throw "llm-clippy: clippy-fork is required"
    else clippy-fork;
  # Drive the toolchain from the fork's `rust-toolchain.toml` so a
  # `nix flake update clippy-fork` advances the rustc/rustc_private ABI in
  # lockstep with the source. If a future fork commit needs different
  # components, edit that file in the fork, not this one.
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
    # Read both the lockfile and the cargo-vendor inputs straight from the fork
    # so `nix flake update clippy-fork` brings dependency changes along with the
    # source commit. No checked-in lockfile to drift.
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
      homepage = "https://github.com/indexable-inc/clippy";
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
