{ix, ...}:
ix.cargoUnit.selectBinaryWithTests ix.rustWorkspace.units {
  binary = "gcal";
  # The binary and the Cargo package are named differently; without this the
  # package-keyed checks (clippy, unused deps) would not attach to the
  # rust-gcal-* lanes.
  packageName = "google-calendar-cli";
  meta.mainProgram = "gcal";
}
