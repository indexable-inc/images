{
  id = "polars-mixedbread";
  # The cdylib is built by the shared workspace unit graph (it reuses the
  # `mixedbread` client crate), and default.nix only packages it into a wheel.
  inRustWorkspace = true;
  # Cross-platform unlike search-py/tui-py: the wheel is consumed inside the nix
  # env (the MCP Python session imports it), not redistributed, so darwin needs
  # no install-name fixups, and the cdylib links on macOS via build.rs. That lets
  # `nix build .#polars-mixedbread` work on darwin for local validation.
  flake = true;
  overlay = false;
  packageSet = true;
  # Gate the pure-Python predicate-pushdown test (default.nix passthru.tests) in
  # CI as `checks.<system>.polars-mixedbread-pushdown`.
  passthruTests = {
    prefix = "polars-mixedbread";
  };
}
