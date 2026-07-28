{
  id = "chromium-cookies";
  packageSet = true;
  flake = true;
  # A local operator CLI for syncing browser sessions onto a VM; nothing
  # consumes it as `pkgs.chromium-cookies` from modules.
  overlay = false;
  inRustWorkspace = true;
  passthruTests = true;
}
