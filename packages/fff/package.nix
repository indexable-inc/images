{
  id = "fff";
  # fff-mcp builds on Linux and macOS (see meta.platforms in default.nix), so
  # surface it on every system: as `pkgs.fff` in the repo package set and as the
  # `fff` flake output.
  packageSet = true;
  flake = true;
}
