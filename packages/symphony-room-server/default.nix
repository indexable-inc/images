{
  lib,
  pkgs,
}:

let
  src = pkgs.fetchFromGitHub {
    owner = "indexable-inc";
    repo = "symphony";
    rev = "9f551287ac587982032bb50fb63dc7ccb492ed6d";
    hash = "sha256-dhNj9J2UL/Rbn9zosPT2n6Cb10jJasraSgDP5rEALs0=";
  };

  roomServer = pkgs.rustPlatform.buildRustPackage {
    pname = "symphony-room-server";
    version = "0.1.0-9f55128";

    cargoLock.lockFile = ./Cargo.lock;
    inherit src;
    cargoBuildFlags = [
      "-p"
      "room-server"
    ];
    doCheck = false;

    meta.mainProgram = "room-server";
  };
in
pkgs.runCommand "symphony-room-server"
  {
    nativeBuildInputs = [ pkgs.makeWrapper ];

    meta = (roomServer.meta or { }) // {
      description = "Room server used by Symphony workflow VMs";
      homepage = "https://github.com/indexable-inc/symphony";
      mainProgram = "room-server";
    };
  }
  ''
    mkdir -p "$out/bin"
    mkdir -p "$out/libexec"
    makeWrapper ${lib.getExe pkgs.codex} "$out/libexec/codex-yolo" \
      --add-flags --dangerously-bypass-approvals-and-sandbox
    makeWrapper ${roomServer}/bin/room-server "$out/bin/room-server" \
      --prefix PATH : ${lib.makeBinPath [ pkgs.codex ]} \
      --set-default ROOM_CODEX_BIN "$out/libexec/codex-yolo"
  ''
