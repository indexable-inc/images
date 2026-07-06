{
  id = "mirror";
  packageSet = true;
  flake = true;
  # CI-only sync tool; nothing consumes it as `pkgs.mirror` from modules.
  overlay = false;
  inRustWorkspace = true;
  passthruTests = true;
}
