{
  ix,
  lib,
  pkgs,
  stdenvNoCC,
  makeWrapper,
  souffle,
}:

let
  unwrapped = ix.cargoUnit.selectBinaryWithTests ix.rustWorkspace.units {
    binary = "scipql";
    meta = {
      description = "Soufflé datalog + find/replace over a SCIP semantic index";
      license = lib.licenses.mit;
      mainProgram = "scipql";
    };
  };

  # Bake the repo's pinned toolchain (same channel as rust-toolchain.toml) so
  # `scipql index` runs the exact rust-analyzer the repo builds against, with
  # rust-src for sysroot analysis, plus cargo/rustc for `cargo metadata`.
  toolchainFile = lib.importTOML ../../../rust-toolchain.toml;
  nightlyDate = lib.removePrefix "nightly-" toolchainFile.toolchain.channel;
  rustToolchain = ix.languages.rust.toolchain pkgs {
    channel = "nightly";
    version = nightlyDate;
    components = [
      "cargo"
      "rustc"
      "rust-std"
      "rust-src"
      "rust-analyzer"
    ];
  };
in
stdenvNoCC.mkDerivation {
  pname = "scipql";
  inherit (unwrapped) version;

  dontUnpack = true;
  strictDeps = true;
  nativeBuildInputs = [ makeWrapper ];

  installPhase = ''
    runHook preInstall
    mkdir -p "$out/bin"
    # Prefix, not suffix: scipql must use its own pinned rust-analyzer/cargo/
    # souffle, never an ambient rustup shim that would shadow them on PATH.
    makeWrapper ${lib.getExe unwrapped} "$out/bin/scipql" \
      --prefix PATH : ${
        lib.makeBinPath [
          rustToolchain
          souffle
        ]
      }
    runHook postInstall
  '';

  meta = {
    description = "Soufflé datalog + find/replace over a SCIP semantic index";
    license = lib.licenses.mit;
    mainProgram = "scipql";
  };
}
