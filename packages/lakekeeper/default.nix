# Lakekeeper, the Apache Iceberg REST catalog (Rust), shipped as an upstream
# prebuilt binary like vector-bin.
{
  autoPatchelfHook,
  fetchzip,
  lib,
  stdenv,
}:
let
  system = stdenv.hostPlatform.system;
  # Add a target here, with its own release hash, before building on another arch.
  targets = {
    x86_64-linux = "x86_64-unknown-linux-gnu";
  };
  target = targets.${system} or (throw "lakekeeper: unsupported system ${system}");
in
stdenv.mkDerivation (finalAttrs: {
  pname = "lakekeeper";
  version = "0.12.3";

  # Upstream ships a single bare `lakekeeper` binary in the tarball (no wrapping
  # directory), so stripRoot must stay off.
  src = fetchzip {
    url = "https://github.com/lakekeeper/lakekeeper/releases/download/v${finalAttrs.version}/lakekeeper-${target}.tar.gz";
    hash = "sha256-vb+LPLtlpJeKC1HbT70Yrb24SdGTknd09C/MPv/yF1U=";
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
})
