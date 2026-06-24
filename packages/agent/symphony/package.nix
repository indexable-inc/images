# Registry metadata. The launcher is a flake output (`nix run .#symphony`,
# `index.packages.<sys>.symphony`, the attr ix's symphony host modules
# consume) and deliberately not an overlay. The room-server package belongs
# beside its source in the ix monorepo. TODO: re-add it once the ix<->index
# flake cycle is resolved. See flake.nix and lib/overlay.nix.
{
  id = "symphony";
  packageSet = true;
  flake = true;
  overlay = false;
}
