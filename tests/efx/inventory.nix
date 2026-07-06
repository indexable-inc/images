# Mini inventory fixture for the efx terranix-port tests: the same node
# shape ix's nix/terraform stacks derive their records and servers from
# (provider, monitoring flag, OVH service name, public addresses), with
# TEST-NET addresses and dummy service ids. Small on purpose — just enough
# nodes to exercise every derivation path the stacks use.
{
  nodes = {
    hel-leader-1 = {
      provider = "hetzner";
      monitoring = true;
      network = {
        publicIpv4 = "192.0.2.10";
        tailscaleIpv4 = "100.64.0.5";
      };
    };
    ord-storage-1 = {
      provider = "ovh";
      monitoring = true;
      hardware.ovh.serviceName = "ns5009988.ip-198-51-100.test";
      network.publicIpv4 = "198.51.100.20";
    };
    vin-compute-1 = {
      provider = "ovh";
      monitoring = false;
      hardware.ovh.serviceName = "ns5011224.ip-203-0-113.test";
      network.publicIpv4 = "203.0.113.40";
    };
  };
}
