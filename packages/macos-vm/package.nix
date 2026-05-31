{
  id = "macos-vm";
  inRustWorkspace = true;
  # macOS-only: the binary links Apple's Virtualization.framework. On Linux the
  # crate still compiles (as a typed "macOS only" stub, so the workspace unit
  # graph stays green there), but the package output and package-set attr are
  # only advertised on Darwin so `nix flake check` never forces an off-platform
  # build. Apple Silicon only for now: the boot path is exercised with an arm64
  # kernel image and Hypervisor.framework on this hardware.
  flake.systems = [ "aarch64-darwin" ];
  packageSet.systems = [ "aarch64-darwin" ];
  passthruTests = true;
}
