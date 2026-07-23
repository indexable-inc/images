# Minimal ix VM base image. `lib/image/oci-layer.nix` auto-enables the shared
# base profile for every image, so this module names the publish target and
# the guest identity.
{ lib, ... }:
{
  ix.image.name = "ix/base";

  # Brand the standalone guest: without this the hostname falls back to the
  # NixOS option default (`config.system.nixos.distroId`, "nixos"), so a VM
  # booted from ix/base (`ix new`) prompts as root@nixos. This lives in the
  # image, not lib/image/platform.nix: the option's own default merges at
  # mkOptionDefault priority (a platform mkOptionDefault would conflict with
  # it), and a platform mkDefault would conflict with the fleet module's
  # per-node `mkDefault name`. mkDefault keeps images layered on ix/base free
  # to rename themselves normally.
  networking.hostName = lib.mkDefault "ix";
}
