{ index }:

# `mkDev` consumes the ix module in ./ix.nix and returns the same shape as
# `mkFleet`. In this repo, example discovery imports this default.nix and
# exposes `nix run .#dev-fleet-up`; a copied flake can expose the returned
# `nixosConfigurations` for `ix up`. `src = ./.` is what a consumer flake passes
# as `self`; it gets materialized at /ix on every node for recursion.
index.lib.mkDev {
  module = ./ix.nix;
  src = ./.;
}
