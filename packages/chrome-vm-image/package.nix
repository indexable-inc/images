{
  id = "chrome-vm-image";
  # aarch64-linux only: the raw EFI disk the `chrome-vm` demo boots under
  # vmkit/libkrun on Apple Silicon. Gate the flake output + package-set attr to
  # that system so `nix flake check` never forces it elsewhere. On hydra it builds
  # via the OrbStack aarch64-linux remote builder.
  flake.systems = [ "aarch64-linux" ];
  packageSet.systems = [ "aarch64-linux" ];
}
