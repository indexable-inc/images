# Target platform applied to every image.
#
# All images run on EPYC Gen 5 (Turin, Zen 5). Setting hostPlatform.gcc.arch
# propagates -march=znver5 -mtune=znver5 to every package in the closure.
# No binary cache hits: everything builds from source.
{ config, lib, ... }:
{
  options.ix.networking = {
    eastWest = {
      hostName = lib.mkOption {
        type = lib.types.str;
        default = config.networking.hostName;
      };
      firewall.allowedTCPPorts = lib.mkOption {
        type = lib.types.listOf lib.types.port;
        default = [ ];
      };
    };

    northSouth.firewall = {
      allowedTCPPorts = lib.mkOption {
        type = lib.types.listOf lib.types.port;
        default = [ ];
      };
      allowedUDPPorts = lib.mkOption {
        type = lib.types.listOf lib.types.port;
        default = [ ];
      };
    };
  };

  config = {
    nixpkgs.hostPlatform = {
      system = "x86_64-linux";
      # TODO: add back znver5 tuning for EPYC Gen 5
      # gcc = {
      #   arch = "znver5";
      #   tune = "znver5";
      # };
    };

    boot.isContainer = true;
    # ix wires VM networking and ingress outside the guest. The stock NixOS
    # DHCP/firewall units expect kernel packet-filtering and BPF/ARP support
    # that ix images do not need and may not expose, which delays boot.
    networking = {
      useDHCP = false;
      firewall.enable = false;
    };
    system.stateVersion = "25.05";
  };
}
