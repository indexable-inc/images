{
  id = "agent-plugin";
  packageSet = true;
  flake = true;
  # Flake-output only: the plugin is agent-config plumbing consumers reach
  # through the index package set (home-manager modules, wrapper overrides),
  # not a tool that belongs in `pkgs`.
  overlay = false;
}
