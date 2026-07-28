{
  id = "nix-web-monitor";
  packageSet = true;
  flake = true;
  overlay = true;
  inRustWorkspace = true;
  cross = true;
  passthruTests = true;
}
