# Switch-target NixOS module shared by every VM in this example.
#
# It is never booted directly: each VM boots from the `ix/base` NixOS image,
# then `ix up` activates this closure in place, the same contract as
# `nixos-rebuild switch`. Importing `virtualisation/docker-image.nix` lets the
# toplevel evaluate without a real bootloader or `fileSystems`, matching how the
# ix base image is built.
#
# This is an ordinary NixOS module: `services.*`, `users.*`, and `systemd.*` all
# work here. Per-VM differences in this example come from the package list in
# `flake.nix`; share everything else from here.
{modulesPath, ...}: {
  imports = [
    "${modulesPath}/virtualisation/docker-image.nix"
  ];

  documentation.enable = false;
  system.stateVersion = "25.11";
}
