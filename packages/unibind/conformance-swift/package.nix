{
  id = "unibind-conformance-swift";
  inRustWorkspace = true;
  # The staticlib itself builds everywhere (the glue is plain Rust); the
  # generated-Swift package and the compile-and-run conformance check need a
  # Swift toolchain, which nixpkgs only caches for darwin, so the package
  # and its passthru tests are darwin-gated.
  packageSet.systems = [
    "aarch64-darwin"
    "x86_64-darwin"
  ];
  # The compile-and-run conformance check surfaces as
  # `checks.<darwin system>.unibind-conformance-swift-conformance` through
  # an explicit darwin-gated merge in lib/per-system.nix (the passthruTests
  # catalog is x86_64-linux-only).
}
