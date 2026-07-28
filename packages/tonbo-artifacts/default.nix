{
  ix,
  lib,
  stdenvNoCC,
  fetchurl,
  # Writer for `passthru.updateScript` (flake-package path only); null on the
  # overlay path.
  # The fork client rather than stock `pkgs.nix`; see packages/yc/default.nix
  # for why an updater must not pull nixpkgs' nix into its closure. Empty on
  # the overlay path, which omits the updateScript anyway.
  repoPackages ? {},
  updateScriptWriter ? null,
}: let
  # Version + URL and SRI hash live in the sibling pins.json, never inline
  # (repo policy). Bump the version/url in pins.json, then `nix run .#update`
  # re-pins the hash.
  pin = ix.pins.loadPin ./pins.json "artifacts";
  inherit (pin) version;
  updateScript = ix.pins.mkOptionalUpdater {
    writeNushellApplication = updateScriptWriter;
    nix = repoPackages.nix-ix;
    pname = "tonbo-artifacts";
    relPath = "packages/tonbo-artifacts/pins.json";
  };
in
  stdenvNoCC.mkDerivation {
    pname = "tonbo-artifacts";
    inherit version;

    src = fetchurl {inherit (pin) url hash;};

    passthru = lib.optionalAttrs (updateScript != null) {inherit updateScript;};

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
      platforms = ["x86_64-linux"];
    };
  }
