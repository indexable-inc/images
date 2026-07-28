{
  id = "bossbar";
  flake = true;
  overlay = false;
  packageSet = true;
  # The flake output builds the CLI wrapper, not the Tauri overlay app; the
  # desktop app stays a `bun run tauri dev`/`build` target.
  path = ./cli.nix;
}
