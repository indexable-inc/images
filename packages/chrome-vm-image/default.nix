# Build a raw EFI-bootable aarch64 NixOS disk image for the `chrome-vm` demo.
# The guest (./nixos.nix) screenshots a baked page with Chromium and base64s the
# PNG over the serial console; the host runner (`packages/chrome-vm`,
# `nix run .#chrome-vm`) boots this under vmkit/libkrun and decodes the shot.
#
# The image is assembled with systemd-repart (the image/repart module imported by
# ./nixos.nix), which runs in the build sandbox with no qemu/kvm VM. So unlike
# `make-disk-image`, this builds on a plain aarch64-linux builder with no
# /dev/kvm, e.g. hydra's OrbStack remote builder. `path` is `pkgs.path` from the
# package autoArgs.
{
  path,
}:
let
  nixos = import "${path}/nixos/lib/eval-config.nix" {
    system = "aarch64-linux";
    modules = [ ./nixos.nix ];
  };
in
# Expose the raw disk directly as the package output (the repart module produces
# it at `${system.build.image}/${image.filePath}`).
nixos.pkgs.runCommand "chrome-vm.raw" { } ''
  cp "${nixos.config.system.build.image}/${nixos.config.image.filePath}" "$out"
''
