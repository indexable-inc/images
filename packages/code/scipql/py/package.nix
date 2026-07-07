{
  id = "scipql-py";
  inRustWorkspace = true;
  # pyo3 extension-module cdylib: the shared workspace injects the darwin
  # `dynamic_lookup` link args for it (replacing the old build.rs), and
  # `unibind.lib.build` assembles the wheel/module/stubs from the built
  # cdylib.
  pyExtension = true;
  # The wheel packaging is Linux-only (patchelf + manylinux tags), matching
  # search-py; on macOS the mcp bundles the module output straight from the
  # workspace graph instead, so darwin needs no packageSet/flake entry. The
  # pyStrict gate (platform-independent) rides the Linux entries.
  flake.systems = [
    "x86_64-linux"
    "aarch64-linux"
  ];
  packageSet.systems = [
    "x86_64-linux"
    "aarch64-linux"
  ];
  # Gate the strict Python type/annotation check (passthru.tests.pyStrict) in
  # CI as `checks.<system>.scipql-py-pyStrict`.
  passthruTests = {
    prefix = "scipql-py";
  };
}
