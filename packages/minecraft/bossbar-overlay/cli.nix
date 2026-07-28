{
  lib,
  stdenvNoCC,
  makeWrapper,
  sqlite,
  coreutils,
}:
# `nix run .#bossbar -- <cmd>` wrapper around the hand-written `bossbar` script,
# which speaks to the overlay's SQLite DB. The script shells out to `sqlite3`
# and a few coreutils (`uname`, `mkdir`, `dirname`), so wrap them onto PATH
# instead of trusting the caller's environment.
stdenvNoCC.mkDerivation {
  pname = "bossbar";
  version = "0.1.0";

  src = ./bossbar;
  dontUnpack = true;

  strictDeps = true;
  nativeBuildInputs = [makeWrapper];

  installPhase = ''
    # shell
    runHook preInstall
    install -Dm755 $src $out/bin/bossbar
    patchShebangs $out/bin/bossbar
    wrapProgram $out/bin/bossbar \
      --prefix PATH : ${
      lib.makeBinPath [
        sqlite
        coreutils
      ]
    }
    runHook postInstall
  '';

  meta = {
    description = "CLI for the Minecraft Boss Bar Overlay's SQLite database";
    mainProgram = "bossbar";
    license = lib.licenses.mit;
    platforms = lib.platforms.unix;
  };
}
