{
  lib,
  pkgs,
  ix,
}: let
  pin = (ix.pins.loadPins ./pins.json).ix-cli;
in
  pkgs.stdenvNoCC.mkDerivation {
    pname = "ix-cli";
    version = "unstable-2026-08-02";
    src = pkgs.fetchurl {
      inherit (pin) hash url;
    };
    dontUnpack = true;
    dontStrip = true;
    strictDeps = true;

    installPhase = ''
      runHook preInstall
      install -Dm755 "$src" "$out/bin/ix"
      runHook postInstall
    '';

    meta = {
      description = "ix command-line client pinned from the public release channel";
      homepage = "https://ix.dev";
      license = lib.licenses.asl20;
      mainProgram = "ix";
      platforms = ["x86_64-linux"];
    };
  }
