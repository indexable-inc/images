{
  id = "panes-compositor";
  inRustWorkspace = true;
  # Guest-side only: runs inside the aarch64-linux VM.
  flake.systems = [ "aarch64-linux" ];
  packageSet.systems = [ "aarch64-linux" ];
  passthruTests = true;
}
