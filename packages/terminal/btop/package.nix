{
  id = "btop";
  packageSet = true;
  flake = true;
  overlay = false;
  # Joins `nix run .#update`: bump btop-src and regenerate the patch series via
  # passthru.updateScript (see default.nix / lib/fork-updater.nix).
  updateScript = true;
}
