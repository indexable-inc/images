{
  lib,
  makeWrapper,
  pkgs,
  rustToolchain ? null,
}:

let
  src = pkgs.fetchFromGitHub {
    owner = "indexable-inc";
    repo = "clippy";
    rev = "c5f8f62dacfc666fa29615b13f777bb7404a1e60";
    hash = "sha256-pFGUPLgM0lSDz8Iv3FLapQAJJV507B1DmJp4pKxp6JA=";
  };

  toolchain =
    if rustToolchain != null then
      rustToolchain
    else
      pkgs.rust-bin.fromRustupToolchainFile (src + "/rust-toolchain.toml");

  rustPlatform = pkgs.makeRustPlatform {
    cargo = toolchain;
    rustc = toolchain;
  };

  rustcLibPathVar =
    if pkgs.stdenv.hostPlatform.isDarwin then "DYLD_LIBRARY_PATH" else "LD_LIBRARY_PATH";
in
rustPlatform.buildRustPackage {
  pname = "llm-clippy";
  version = "0.1.97";

  inherit src;
  cargoLock.lockFile = ./Cargo.lock;
  # Upstream indexable-inc/clippy ships no Cargo.lock. cargoLock.lockFile only
  # vendors and validates ("ERROR: Missing Cargo.lock from src" if it isn't
  # present), so cargoPatches is how we plant the file into $sourceRoot.
  cargoPatches = [ ./cargo-lock.patch ];

  nativeBuildInputs = [ makeWrapper ];
  buildInputs = [
    pkgs.zlib
  ]
  ++ lib.optionals pkgs.stdenv.hostPlatform.isDarwin [
    pkgs.libiconv
  ];
  doCheck = false;

  # This Clippy fork links against rustc_private crates from its Rust toolchain.
  RUSTC_BOOTSTRAP = "1";

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
