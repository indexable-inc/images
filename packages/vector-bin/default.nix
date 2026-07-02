{
  autoPatchelfHook,
  fetchzip,
  ix,
  lib,
  nix,
  stdenv,
  # Writer for `passthru.updateScript`, bound only on the flake-package path
  # (lib/packages.nix); the overlay path leaves it null so `pkgs.*` carries no
  # updater. Same nullable-writer pattern as claude-code / yc.
  updateScriptWriter ? null,
}:
let
  # Prebuilt binary is x86_64-linux only; the package-set/flake targets and
  # meta.platforms below gate that, so the unsupported-system throw is redundant.
  targets = {
    x86_64-linux = "x86_64-unknown-linux-gnu";
  };
  # Version + per-release URL and SRI hash live in the sibling pins.json, never
  # inline here (repo policy: no `hash = "sha256-..."` literals in tracked .nix).
  # Bump the version/url in pins.json, then `nix run .#update` re-pins the hash.
  pin = ix.pins.loadPin ./pins.json "vector";
  updateScript =
    if updateScriptWriter == null then
      null
    else
      ix.pins.mkUpdater {
        writeNushellApplication = updateScriptWriter;
        inherit nix;
        pname = "vector-bin";
        relPath = "packages/vector-bin/pins.json";
      };
in
stdenv.mkDerivation {
  pname = "vector";
  inherit (pin) version;

  src = fetchzip { inherit (pin) url hash; };

  passthru = lib.optionalAttrs (updateScript != null) { inherit updateScript; };

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
