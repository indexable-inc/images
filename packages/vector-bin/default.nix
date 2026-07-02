{
  autoPatchelfHook,
  fetchzip,
  ix,
  lib,
  stdenv,
}:
let
  # Prebuilt binary is x86_64-linux only; the package-set/flake targets and
  # meta.platforms below gate that, so the unsupported-system throw is redundant.
  targets = {
    x86_64-linux = "x86_64-unknown-linux-gnu";
  };
  # Version + per-release URL and SRI hash live in the sibling pins.json, never
  # inline here (repo policy: no `hash = "sha256-..."` literals in tracked .nix).
  # Bump with `nix run .#update`.
  pin = ix.pins.loadPin ./pins.json "vector";
in
stdenv.mkDerivation {
  pname = "vector";
  inherit (pin) version;

  src = fetchzip { inherit (pin) url hash; };

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
}
