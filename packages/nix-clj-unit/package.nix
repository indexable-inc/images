{
  id = "nix-clj-unit";
  packageSet = true;
  flake = true;
  overlay = false;
  inRustWorkspace = true;
  passthruTests = true;
}
