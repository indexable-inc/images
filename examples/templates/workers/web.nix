# The named half of the config. `web` is declared the way every other example
# declares a VM, and `ix apply` treats it and the two rendered instances
# identically -- which is the point of it being here.
#
# It does not proxy to the workers, and in this version it cannot: instances
# are rendered one layer above the config (flake.nix), so nothing inside
# default.ix can hand their `nixosConfigurations` to a peer's `nodes` argument.
# RFC 0042 leaves cross-instance wiring out of v1 deliberately; a named VM
# wired to other NAMED VMs works today, as examples/multi-vm/microservices
# shows.
_: let
  httpPort = 80;
in {
  services.nginx = {
    enable = true;
    virtualHosts.web = {
      default = true;
      listen = [
        {
          addr = "0.0.0.0";
          port = httpPort;
        }
      ];
      locations."/".return = "200 'templates-workers: web is named, the workers are instances\n'";
    };
  };

  ix.networking.expose.http = {
    port = httpPort;
    description = "public entrypoint";
  };

  ix.healthChecks.nginx.unit = "nginx";
}
