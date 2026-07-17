{
  id = "nix-kbuild-unit";
  packageSet = true;
  flake = true;
  overlay = false;
  # Not inRustWorkspace: like nix-cargo-unit, this crate is a standalone Cargo
  # workspace (own Cargo.toml + Cargo.lock), built as a plain package and kept
  # out of the root workspace unit graph. passthruTests still gate on
  # packageSet.
  passthruTests = true;
}
