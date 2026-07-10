# Minecraft Bedrock Dedicated Server package. Kept in its own callPackage file
# (explicit deps, no `pkgs` arg) so it stays `override`-able and the module that
# consumes it does not build a derivation inline.
{
  ix,
  stdenv,
  fetchurl,
  autoPatchelfHook,
  unzip,
  curl,
  glibc,
}: let
  # Version + download URL and SRI hash live in the sibling pins.json, never
  # inline (repo policy: no hash literals in tracked .nix).
  pin = ix.pins.loadPin ./pins.json "bedrock-server";
in
  stdenv.mkDerivation {
    pname = "minecraft-bedrock-server";
    inherit (pin) version;

    src = fetchurl {
      inherit (pin) url hash;
      curlOptsList = [
        "--http1.1"
        "-A"
        "Mozilla/5.0"
      ];
    };

    strictDeps = true;
    # The bedrock zip has no wrapper directory: files land directly in $PWD.
    # Without this, Nix's unpackPhase tries to auto-detect a single extracted
    # directory to cd into, and fails because it finds multiple entries instead.
    sourceRoot = ".";
    nativeBuildInputs = [
      autoPatchelfHook
      unzip
    ];
    buildInputs = [
      curl
      glibc
      stdenv.cc.cc.lib
    ];
    dontConfigure = true;
    dontBuild = true;

    installPhase = ''
      # shell
      runHook preInstall

      mkdir -p "$out/bin" "$out/share/minecraft-bedrock-server"
      cp -R . "$out/share/minecraft-bedrock-server/"
      chmod +x "$out/share/minecraft-bedrock-server/bedrock_server"
      ln -s "$out/share/minecraft-bedrock-server/bedrock_server" "$out/bin/bedrock_server"

      runHook postInstall
    '';

    meta.mainProgram = "bedrock_server";
  }
