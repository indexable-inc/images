{
  id = "panes-guest-image";
  # aarch64-linux only: the raw EFI disk the panes seamless-windows guest boots
  # under vmkit/libkrun on Apple Silicon (index#1686). Gate the flake output and
  # package-set attr to that system so `nix flake check` never forces it
  # elsewhere, same as packages/vm/chrome-vm-image.
  flake.systems = ["aarch64-linux"];
  packageSet.systems = ["aarch64-linux"];
}
