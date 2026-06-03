{
  id = "nix-cargo-unit";
  packageSet = true;
  flake = true;
  # Not inRustWorkspace: this crate is a standalone Cargo workspace (own
  # Cargo.toml + Cargo.lock), so it is built as a plain package and kept out of
  # the root workspace unit graph. passthruTests still gate on packageSet.
  passthruTests = true;
}
