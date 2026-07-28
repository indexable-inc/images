{
  id = "cursor-cli";
  packageSet = true;
  # Flake-output only, deliberately NOT an overlay (same posture as codex):
  # `pkgs.cursor-cli` stays the plain nixpkgs CLI; our wrapper is an additive
  # output (`nix run .#cursor-cli`, `index.packages.<sys>.cursor-cli`) that
  # bakes house defaults on top of that same base.
  flake = true;
  overlay = false;
}
