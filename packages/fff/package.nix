{
  id = "fff";
  # fff-mcp builds on Linux and macOS (see meta.platforms in default.nix), so
  # surface it on every system: as `pkgs.fff` in the repo package set and as the
  # `fff` flake output. `overlay` also threads it into the nixpkgs overlay so
  # other repo packages can take `pkgs.fff` as an input: `mcp` bundles the
  # fff-c cdylib emitted here for its ctypes-backed `import fff`.
  packageSet = true;
  flake = true;
  overlay = true;
}
