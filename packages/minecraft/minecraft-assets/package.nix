{
  id = "minecraft-assets";
  flake = true;
  packageSet = true;
  # Exposed in the package overlay so the overlay derivations can `callPackage` it.
  overlay = true;
}
