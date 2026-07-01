{
  id = "panes-protocol";
  inRustWorkspace = true;
  # Library crate: consumed by panes-compositor (aarch64-linux) and panes-host
  # (aarch64-darwin) through the workspace unit graph, like ast-merge-ast. No
  # flake/packageSet systems: advertising them would require a default.nix
  # target selection for a crate with no standalone artifact.
  passthruTests = true;
}
