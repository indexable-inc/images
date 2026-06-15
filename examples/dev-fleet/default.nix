{ index }:

# `mkDev` consumes the forkable spec in ./dev.nix and returns the same shape as
# `mkFleet`, so `ix up` / `nix run .#dev-fleet-up` work unchanged. `src = ./.`
# is what the template's flake.nix passes as the flake `self`; it is what gets
# materialized at /ix on every node for recursion.
index.lib.mkDev (import ./dev.nix // { src = ./.; })
