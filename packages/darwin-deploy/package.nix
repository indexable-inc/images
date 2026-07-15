{
  id = "darwin-deploy";
  packageSet = true;
  flake = true;
  # Deploy CLI invoked by operators and modules' deploy scripts; nothing
  # consumes it as `pkgs.darwin-deploy` from modules yet.
  overlay = false;
  inRustWorkspace = true;
  passthruTests = true;
}
