{
  id = "vz-linux-guest";
  # aarch64-linux only: this is the raw EFI disk the vmkit `boot-linux-gui`
  # path boots under Apple Virtualization.framework on Apple Silicon. Gate the
  # flake output and package-set attr to that system so `nix flake check` never
  # forces it elsewhere.
  flake.systems = ["aarch64-linux"];
  packageSet.systems = ["aarch64-linux"];
}
