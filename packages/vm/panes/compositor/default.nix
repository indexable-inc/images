# The compositor binary out of the shared workspace unit graph (dag-runner
# pattern): package.nix carries the registry metadata, this file only selects
# the target. packages/vm/panes/guest-image/default.nix selects the same unit
# inline today and has a TODO to switch to `repoPackages.panes-compositor`.
{ix, ...}:
ix.cargoUnit.selectBinaryWithTests ix.rustWorkspace.units {
  binary = "panes-compositor";
  meta = {
    description = "Guest-side headless Wayland compositor exporting toplevels over vsock";
    mainProgram = "panes-compositor";
    platforms = ["aarch64-linux"];
  };
}
