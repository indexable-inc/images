{ index }:

# `mkDev` consumes the ix module in ./ix.nix and returns the same shape as
# `mkFleet`, so `ix up` / `nix run .#dev-fleet-up` work unchanged. `src = ./.`
# is what a consumer flake passes as `self`; it gets materialized at /ix on
# every node for recursion.
index.lib.mkDev {
  module = ./ix.nix;
  src = ./.;
}
