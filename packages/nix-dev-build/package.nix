{
  id = "nix-dev-build";
  packageSet = true;
  flake = true;
  overlay = false;
  inRustWorkspace = true;
  passthruTests = true;
}
