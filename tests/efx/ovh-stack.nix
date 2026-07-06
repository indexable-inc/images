# Terranix-shaped OVH monitoring stack, ported from ix's nix/terraform/ovh:
# one ovh_dedicated_server resource per inventory node whose provider is
# "ovh" plus the unassigned pool capacity, exactly the derivation the
# terraform original ran. The `terraform` / `provider` / `import` blocks stay
# in the fixture so the translator's drop rules are exercised: state moved to
# the efx journal, auth to the executor environment, and import blocks have
# no efx meaning (executors reconcile against live state instead).
{
  lib,
  inventory,
}: let
  ovhNodes = lib.filterAttrs (_: node: node.provider == "ovh") inventory.nodes;
  unassignedServers = {
    shared-compute-1 = {
      serviceName = "ns1033398.ip-192-0-2.test";
      monitoring = false;
    };
  };
  servers =
    lib.mapAttrs (_: node: {
      serviceName = node.hardware.ovh.serviceName;
      inherit (node) monitoring;
    })
    ovhNodes
    // unassignedServers;
in {
  terraform.required_providers.ovh = {
    source = "ovh/ovh";
    version = "~> 2.0";
  };
  provider.ovh = {};

  resource.ovh_dedicated_server =
    lib.mapAttrs (name: server: {
      service_name = server.serviceName;
      inherit (server) monitoring;
      display_name = name;
      keep_service_after_destroy = true;
      prevent_install_on_create = true;
      prevent_install_on_import = true;
    })
    servers;

  import =
    lib.mapAttrsToList (name: server: {
      to = "ovh_dedicated_server.${name}";
      id = server.serviceName;
    })
    servers;
}
