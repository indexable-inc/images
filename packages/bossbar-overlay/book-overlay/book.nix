{
  lib,
  runCommand,
  bossbar-overlay,
}:
# `.#book-overlay` reuses the single workspace build (which produces *both*
# binaries) and just exposes the book binary as the main program. It must not
# re-derive `bossbar-overlay`: `meta.mainProgram` is injected as the
# `NIX_MAIN_PROGRAM` build env var, so overriding it would change the derivation
# hash and force a full second compile of the whole wgpu/winit workspace. Instead
# this is a trivial symlink derivation depending on the already-built package, so
# there is genuinely no second build.
runCommand "book-overlay"
  {
    meta = bossbar-overlay.meta // {
      description = "Minecraft-style book desktop overlay (wgpu, SQLite-driven)";
      mainProgram = "book-overlay";
    };
  }
  ''
    mkdir -p "$out/bin"
    ln -s ${lib.getExe' bossbar-overlay "book-overlay"} "$out/bin/book-overlay"
  ''
