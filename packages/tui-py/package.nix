{
  id = "tui-py";
  inRustWorkspace = true;
  # The cdylib links on macOS too now (build.rs emits the cdylib-scoped
  # `-undefined dynamic_lookup` that PyO3 needs there), but this wheel's
  # packaging is still Linux-only: it strips an ELF rpath with patchelf and
  # stamps manylinux tags. macOS would need install-name fixups and macosx tags
  # instead, which no caller wants yet. The mcp bundles the cdylib straight from
  # the workspace graph (see packages/mcp) for cross-platform `import tui`, so
  # this Linux wheel stays the distribution artifact and macOS dev uses that.
  flake.systems = [
    "x86_64-linux"
    "aarch64-linux"
  ];
  packageSet.systems = [
    "x86_64-linux"
    "aarch64-linux"
  ];
}
