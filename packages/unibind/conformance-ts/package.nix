{
  id = "unibind-conformance-ts";
  inRustWorkspace = true;
  # A Node addon must not inherit the workspace-unified `py` feature of
  # unibind-runtime (pyo3's #[used] constructors would leave the cdylib
  # with undefined Python symbols at dlopen); resolve this crate's
  # dependency features from its own manifest via a dedicated `-p` cargo
  # invocation in the shared unit graph.
  isolatedFeatures = true;
  # Linux-only, like tui-node: the N-API addon is a cdylib carved from the
  # shared cargo-unit graph, and that graph does not thread macOS's
  # `-undefined dynamic_lookup` through to the link step. Local macOS dev
  # builds the addon with plain `cargo build`, which honors napi-build's
  # link args.
  flake.systems = [
    "x86_64-linux"
    "aarch64-linux"
  ];
  packageSet.systems = [
    "x86_64-linux"
    "aarch64-linux"
  ];
  # Gate the Node end-to-end suite (passthru.tests.node-conformance) in CI
  # as `checks.<system>.unibind-conformance-ts-node-conformance`.
  passthruTests = {
    prefix = "unibind-conformance-ts";
  };
}
