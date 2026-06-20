# Build a raw EFI-bootable aarch64 NixOS disk image for the vmkit
# `boot-linux-gui` path. The image boots into a Wayland compositor running
# `bossbar-overlay` on software graphics (see ./nixos.nix).
#
# `path` is `pkgs.path` (the nixpkgs source) supplied by the package autoArgs;
# `bossbar-overlay` is the overlaid aarch64-linux package. We evaluate a NixOS
# system from nixpkgs' own `eval-config` and render it with `make-disk-image`,
# so no extra flake input (nixos-generators/disko) is needed.
{
  lib,
  path,
  bossbar-overlay,
}:
let
  nixos = import "${path}/nixos/lib/eval-config.nix" {
    system = "aarch64-linux";
    modules = [
      ./nixos.nix
      # Inject the overlaid `bossbar-overlay` through the package set rather than
      # `specialArgs`, so the guest module reads it as `pkgs.bossbar-overlay`.
      { nixpkgs.overlays = [ (_final: _prev: { inherit bossbar-overlay; }) ]; }
    ];
  };
in
import "${path}/nixos/lib/make-disk-image.nix" {
  inherit lib;
  inherit (nixos) config pkgs;
  format = "raw";
  partitionTableType = "efi";
  # Headroom over the closure so the ext4 root is not packed to 100%.
  additionalSpace = "512M";
}
