# NixOS branch of the blocklist: first-class /etc/hosts state via
# `networking.extraHosts`.
{
  lib,
  config,
  ...
}: {
  imports = [./common.nix];
  config = lib.mkIf (config.networking.blockedHosts != []) {
    networking.extraHosts = config.networking.blockedHostsText;
  };
}
