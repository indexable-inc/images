# Lakekeeper, the Apache Iceberg REST catalog (Rust), shipped as an upstream
# prebuilt binary like vector-bin.
{
  autoPatchelfHook,
  fetchzip,
  ix,
  lib,
  stdenv,
}:
let
  # Add a target here, with its own release hash, before building on another
  # arch. The package-set/flake targets and meta.platforms below gate the arch.
  targets = {
    x86_64-linux = "x86_64-unknown-linux-gnu";
  };
  # Version + per-release URL and SRI hash live in the sibling pins.json, never
  # inline (repo policy). Bump with `nix run .#update`.
  pin = ix.pins.loadPin ./pins.json "lakekeeper";
in
stdenv.mkDerivation {
  pname = "lakekeeper";
  inherit (pin) version;

  # Upstream ships a single bare `lakekeeper` binary in the tarball (no wrapping
  # directory), so stripRoot must stay off.
  src = fetchzip {
    inherit (pin) url hash;
    stripRoot = false;
  };

  nativeBuildInputs = [ autoPatchelfHook ];
  buildInputs = [
    stdenv.cc.cc.lib
    stdenv.cc.libc
  ];

  installPhase = ''
    # shell
    runHook preInstall
    install -Dm755 "$src/lakekeeper" "$out/bin/lakekeeper"
    runHook postInstall
  '';

  meta = {
    description = "Apache Iceberg REST Catalog written in Rust";
    homepage = "https://lakekeeper.io";
    license = lib.licenses.asl20;
    mainProgram = "lakekeeper";
    platforms = builtins.attrNames targets;
    sourceProvenance = [ lib.sourceTypes.binaryNativeCode ];
  };
}
