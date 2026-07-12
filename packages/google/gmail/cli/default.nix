{ix, ...}:
ix.cargoUnit.selectBinaryWithTests ix.rustWorkspace.units {
  binary = "gmail";
  # The binary and the Cargo package are named differently; without this the
  # package-keyed checks (clippy, unused deps) would not attach to the
  # rust-gmail-* lanes.
  packageName = "google-gmail-cli";
  meta.mainProgram = "gmail";
}
