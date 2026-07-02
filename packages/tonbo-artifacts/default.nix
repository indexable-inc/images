{
  ix,
  lib,
  nix,
  stdenvNoCC,
  fetchurl,
  # Writer for `passthru.updateScript` (flake-package path only); null on the
  # overlay path.
  updateScriptWriter ? null,
}:

let
  # Version + URL and SRI hash live in the sibling pins.json, never inline
  # (repo policy). Bump the version/url in pins.json, then `nix run .#update`
  # re-pins the hash.
  pin = ix.pins.loadPin ./pins.json "artifacts";
  inherit (pin) version;
  updateScript =
    if updateScriptWriter == null then
      null
    else
      ix.pins.mkUpdater {
        writeNushellApplication = updateScriptWriter;
        inherit nix;
        pname = "tonbo-artifacts";
        relPath = "packages/tonbo-artifacts/pins.json";
      };
in
stdenvNoCC.mkDerivation {
  pname = "tonbo-artifacts";
  inherit version;

  src = fetchurl { inherit (pin) url hash; };

  passthru = lib.optionalAttrs (updateScript != null) { inherit updateScript; };

  dontUnpack = true;
  dontBuild = true;
  strictDeps = true;

  installPhase = ''
    # shell
    runHook preInstall

    install -Dm755 "$src" "$out/bin/artifacts"

    runHook postInstall
  '';

  meta = {
    description = "Tonbo Artifacts CLI";
    homepage = "https://artifacts.tonbo.io/docs/overview/";
    mainProgram = "artifacts";
    platforms = [ "x86_64-linux" ];
  };
}
