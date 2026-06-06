# Registry metadata. The launcher is a flake output (`nix run .#symphony`,
# `index.packages.<sys>.symphony`, the attr ix's symphony host modules
# consume) and deliberately not an overlay: nothing inside an image
# evaluation needs `pkgs.symphony`, and the room-server the symphony-codex
# image embeds is a separate package (`pkgs.symphony-room-server`, still
# provided by the pinned `symphony` flake input).
{
  id = "symphony";
  packageSet = true;
  flake = true;
}
