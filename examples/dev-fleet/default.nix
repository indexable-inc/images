{
  index,
  src ? ./.,
}:

# `mkDev` consumes the ix module in ./ix.nix and returns the same shape as
# `mkFleet`, so `ix up` can consume it. `src` defaults to ./. for direct imports,
# while the standalone flake passes `self`; it gets materialized at /ix on every
# node for recursion.
index.lib.mkDev {
  module = ./ix.nix;
  inherit src;
}
