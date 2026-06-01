{
  lib,
  runCommand,
  bossbar-overlay,
}:
# `.#xp-orb-overlay` reuses the single workspace build (which produces *all* the
# overlay binaries) and just exposes the experience-orb binary as the main
# program. It must not re-derive `bossbar-overlay`: `meta.mainProgram` is injected
# as the `NIX_MAIN_PROGRAM` build env var, so overriding it would change the
# derivation hash and force a full second compile of the whole wgpu/winit
# workspace. Instead this is a trivial symlink derivation depending on the
# already-built package, so there is genuinely no second build.
runCommand "xp-orb-overlay"
  {
    meta = bossbar-overlay.meta // {
      description = "Minecraft-style floating experience-orb desktop overlay (wgpu, SQLite-driven)";
      mainProgram = "xp-orb-overlay";
    };
  }
  ''
    mkdir -p "$out/bin"
    ln -s ${lib.getExe' bossbar-overlay "xp-orb-overlay"} "$out/bin/xp-orb-overlay"
  ''
