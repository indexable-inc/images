{
  ix,
  lib,
  makeWrapper,
  pkgs,
  src,
}:

let
  # Drive the toolchain from the fork's `rust-toolchain.toml` so a
  # `nix flake update clippy-fork` advances the rustc/rustc_private ABI in
  # lockstep with the source. If a future fork commit needs different
  # components, edit that file in the fork, not this one.
  toolchain = pkgs.rust-bin.fromRustupToolchainFile (src + "/rust-toolchain.toml");

  rustcLibPathVar =
    if pkgs.stdenv.hostPlatform.isDarwin then "DYLD_LIBRARY_PATH" else "LD_LIBRARY_PATH";
in
ix.buildRustPackage pkgs {
  pname = "llm-clippy";
  version = "0.1.97";

  inherit src;
  rustToolchain = toolchain;
  # Read both the lockfile and the cargo-vendor inputs straight from the fork
  # so `nix flake update clippy-fork` brings dependency changes along with the
  # source commit. No checked-in lockfile to drift.
  cargoLock.lockFile = src + "/Cargo.lock";

  nativeBuildInputs = [ makeWrapper ];
  buildInputs = [
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
