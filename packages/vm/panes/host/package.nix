{
  id = "panes-host";
  inRustWorkspace = true;
  # Host-side only: AppKit/Metal, Apple Silicon.
  flake.systems = [ "aarch64-darwin" ];
  packageSet.systems = [ "aarch64-darwin" ];
  passthruTests = true;
}
