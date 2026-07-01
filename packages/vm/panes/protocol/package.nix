{
  id = "panes-protocol";
  inRustWorkspace = true;
  # Shared wire types: built (and unit-tested) on both ends of the stream, the
  # aarch64-darwin host agent and the aarch64-linux guest compositor.
  flake.systems = [
    "aarch64-darwin"
    "aarch64-linux"
  ];
  packageSet.systems = [
    "aarch64-darwin"
    "aarch64-linux"
  ];
  passthruTests = true;
}
