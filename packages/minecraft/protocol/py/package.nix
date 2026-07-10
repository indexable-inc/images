{
  id = "mc-protocol-py";
  inRustWorkspace = true;
  # pyo3 extension-module cdylib: the shared workspace injects the darwin
  # `dynamic_lookup` link args for it, and `unibind.lib.build` assembles the
  # wheel/module/stubs from the built cdylib.
  pyExtension = true;
  # The wheel packaging is Linux-only (patchelf + manylinux tags), matching
  # scipql-py; the pyStrict gate (platform-independent) rides the Linux
  # entries.
  flake.systems = [
    "x86_64-linux"
    "aarch64-linux"
  ];
  packageSet.systems = [
    "x86_64-linux"
    "aarch64-linux"
  ];
  # Gate the strict Python type/annotation check (passthru.tests.pyStrict) in
  # CI as `checks.<system>.mc-protocol-py-pyStrict`.
  passthruTests = {
    prefix = "mc-protocol-py";
  };
}
