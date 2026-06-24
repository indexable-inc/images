{
  autoPatchelfHook,
  fetchzip,
  lib,
  stdenv,
}:
let
  system = stdenv.hostPlatform.system;
  targets = {
    x86_64-linux = "x86_64-unknown-linux-gnu";
  };
  target = targets.${system} or (throw "vector-bin: unsupported system ${system}");
in
stdenv.mkDerivation (finalAttrs: {
  pname = "vector";
  version = "0.55.0";

  src = fetchzip {
    url = "https://github.com/vectordotdev/vector/releases/download/v${finalAttrs.version}/vector-${finalAttrs.version}-${target}.tar.gz";
    hash = "sha256-VbmY+8NBcQRxqB8dXkE1P5OlVEFf4V10aN4podxbavs=";
  };

  nativeBuildInputs = [ autoPatchelfHook ];
  buildInputs = [
    stdenv.cc.cc.lib
    stdenv.cc.libc
  ];

  installPhase = ''
    # shell
    runHook preInstall

    install -Dm755 "$src/bin/vector" "$out/bin/vector"
    install -Dm644 "$src/LICENSE" "$out/share/licenses/vector/LICENSE"
    install -Dm644 "$src/NOTICE" "$out/share/doc/vector/NOTICE"

    runHook postInstall
  '';

  meta = {
    description = "High-performance observability data pipeline";
    homepage = "https://vector.dev";
    license = lib.licenses.mpl20;
    mainProgram = "vector";
    platforms = builtins.attrNames targets;
    sourceProvenance = [ lib.sourceTypes.binaryNativeCode ];
  };
})
