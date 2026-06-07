# Registry metadata. The launcher is a flake output (`nix run .#symphony`,
# `index.packages.<sys>.symphony`, the attr ix's symphony host modules
# consume) and deliberately not an overlay: nothing inside an image
# evaluation needs `pkgs.symphony`. The room-server the symphony-codex image
# embeds is a separate package (`pkgs.symphony-room-server`).
# TODO: re-add it; the `symphony` flake input that provided it was removed
# (room-server lives in the ix monorepo; the ix<->index flake cycle blocks
# sourcing it from ix). See flake.nix and lib/overlay.nix.
{
  id = "symphony";
  packageSet = true;
  flake = true;
}
