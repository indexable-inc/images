{
  id = "tui-node";
  inRustWorkspace = true;
  # Linux-only, like tui-py: the N-API addon is a cdylib carved from the shared
  # cargo-unit graph, and that graph does not thread macOS's
  # `-undefined dynamic_lookup` through to the link step. Local macOS dev builds
  # the addon with plain `cargo build`, which honors napi-build's link args.
  flake.systems = [
    "x86_64-linux"
    "aarch64-linux"
  ];
  packageSet.systems = [
    "x86_64-linux"
    "aarch64-linux"
  ];
}
