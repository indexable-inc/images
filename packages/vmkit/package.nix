{
  id = "vmkit";
  inRustWorkspace = true;
  # Cross-host VM driver. aarch64-darwin links Apple's Virtualization.framework
  # (macOS guests) plus libkrun-efi (Linux guests); Linux links classic KVM
  # libkrun (Linux guests). x86_64-darwin is omitted: libkrun-efi is aarch64-only
  # and the macOS-guest boot path is exercised only on Apple Silicon. The crate
  # still compiles everywhere (off-host code is cfg'd out), but the package output
  # and package-set attr are advertised only on supported hosts so `nix flake
  # check` never forces an off-platform build.
  flake.systems = [
    "aarch64-darwin"
    "aarch64-linux"
    "x86_64-linux"
  ];
  packageSet.systems = [
    "aarch64-darwin"
    "aarch64-linux"
    "x86_64-linux"
  ];
  passthruTests = true;
}
