{
  id = "ix-windows";
  # macOS-only for now: wry links the system WebKit framework on darwin, while
  # Linux (WebKitGTK) is a later add (see default.nix). Gating every target to
  # darwin keeps the linux flake/package/rust-test paths from pulling the
  # gtk/webkit2gtk build inputs that are not wired up, so a linux `flake-check`
  # never compiles this crate.
  packageSet.systems = [
    "aarch64-darwin"
    "x86_64-darwin"
  ];
  flake.systems = [
    "aarch64-darwin"
    "x86_64-darwin"
  ];
  inRustWorkspace.systems = [
    "aarch64-darwin"
    "x86_64-darwin"
  ];
  passthruTests = true;
}
