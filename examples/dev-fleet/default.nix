{ index }:

# `mkDev` consumes the ix module in ./ix.nix and returns the same shape as
# `mkFleet`, so `ix up` / `nix run .#dev-fleet-up` work unchanged. `src = ./.`
# is what the template's flake.nix passes as the flake `self`; it is what gets
# materialized at /ix on every node for recursion.
index.lib.mkDev {
  module = ./ix.nix;
  src = ./.;
}
