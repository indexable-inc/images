# Minecraft Bedrock Dedicated Server package. Kept in its own callPackage file
# (explicit deps, no `pkgs` arg) so it stays `override`-able and the module that
# consumes it does not build a derivation inline.
{
  lib,
  stdenv,
  fetchurl,
  autoPatchelfHook,
  unzip,
  curl,
  glibc,
}:
stdenv.mkDerivation {
  pname = "minecraft-bedrock-server";
  version = "1.26.14.1";

  src = fetchurl {
    url = "https://www.minecraft.net/bedrockdedicatedserver/bin-linux/bedrock-server-1.26.14.1.zip";
    hash = "sha256-g9XaCRI8PwtgPFS+kpaOXA5DdbWE1RTWEID2Nuekx3Q=";
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
    runHook preInstall

    mkdir -p "$out/bin" "$out/share/minecraft-bedrock-server"
    cp -R . "$out/share/minecraft-bedrock-server/"
    chmod +x "$out/share/minecraft-bedrock-server/bedrock_server"
    ln -s "$out/share/minecraft-bedrock-server/bedrock_server" "$out/bin/bedrock_server"

    runHook postInstall
  '';

  meta.mainProgram = "bedrock_server";
}
