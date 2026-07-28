{
  id = "search-py";
  inRustWorkspace = true;
  # The cdylib links on macOS too (build.rs emits the cdylib-scoped `-undefined
  # dynamic_lookup` that PyO3 needs there), but this wheel's packaging is still
  # Linux-only: it strips an ELF rpath with patchelf and stamps manylinux tags.
  # macOS would need install-name fixups and macosx tags instead, which no
  # caller wants yet. The mcp bundles the cdylib straight from the workspace
  # graph (see packages/mcp) for cross-platform `import search`, so this
  # Linux wheel stays the distribution artifact and macOS dev uses that.
  flake.systems = [
    "x86_64-linux"
    "aarch64-linux"
  ];
  packageSet.systems = [
    "x86_64-linux"
    "aarch64-linux"
  ];
  # Gate the strict Python type/annotation check (default.nix passthru.tests.pyStrict)
  # in CI as `checks.<system>.search-py-pyStrict`.
  passthruTests = {
    prefix = "search-py";
  };
}
