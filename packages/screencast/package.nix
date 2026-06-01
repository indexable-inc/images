{
  id = "screencast";
  # macOS-only: avfoundation capture and hevc_videotoolbox are Darwin APIs (the
  # crate's meta.platforms is lib.platforms.darwin). Advertise the package output,
  # package-set attr, and passthru checks only on Darwin so `nix flake check` on
  # x86_64-linux never forces the off-platform build. Mirrors macos-vm.
  packageSet.systems = [
    "aarch64-darwin"
    "x86_64-darwin"
  ];
  flake.systems = [
    "aarch64-darwin"
    "x86_64-darwin"
  ];
  inRustWorkspace = true;
  passthruTests = true;
}
