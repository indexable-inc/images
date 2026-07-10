{
  id = "codex";
  # `packageSet` here means the index package set (`index.packages.<sys>.codex`,
  # built via packageSetFor), NOT a nixpkgs overlay: it does not inject into
  # `pkgs`, so `pkgs.codex` stays the plain nixpkgs CLI (see the `flake`-only
  # note below).
  packageSet = true;
  # Flake-output only, deliberately NOT an overlay: `pkgs.codex` must stay the
  # plain nixpkgs CLI because symphony's room-server wrapper pins `pkgs.codex`
  # as the binary it spawns over JSON-RPC. Our wrapper is an additive output
  # (`nix run .#codex`, `index.packages.<sys>.codex`) that bakes our defaults on
  # top of that same base, without changing what the overlay hands other code.
  flake = true;
  overlay = false;
}
