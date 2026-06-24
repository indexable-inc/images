{
  id = "nix-web-monitor";
  packageSet = true;
  flake = true;
  overlay = true;
  inRustWorkspace = true;
  passthruTests = true;
}
