# The template's guest definition. Everything instance-specific arrives in the
# `worker` module argument that default.ix builds from the params, so this file
# is the same for every instance and nothing in it names an instance.
{
  name,
  worker,
  ...
}: {
  services.nginx = {
    enable = true;
    # nginx's own shard count, and the reason `shards` is a param rather than a
    # constant. `appendConfig` is nixpkgs' seam for a main-context directive;
    # there is no typed option for this one.
    appendConfig = "worker_processes ${toString worker.shards};";
    virtualHosts.worker = {
      default = true;
      listen = [
        {
          addr = "0.0.0.0";
          inherit (worker) port;
        }
      ];
      locations = {
        # Stand-in for a real service: each instance reports the identity and
        # the params it was rendered from, so a request says which instance
        # answered and with what configuration.
        "/" = {
          return = "200 '{\"node\":\"${name}\",\"instance\":\"${worker.instance}\",\"shards\":${toString worker.shards}}\n'";
          extraConfig = "default_type application/json;";
        };
        # A cheap dedicated readiness route for probes.
        "/healthz".return = "200 'ok\n'";
      };
    };
  };

  # One declaration opens the firewall, registers the port claim, and lets a
  # peer resolve this listener by name. The claim is scoped to one VM, so two
  # instances given the same `port` are fine -- they are separate machines;
  # what it catches is a second module on THIS VM claiming the same port.
  ix.networking.expose.http = {
    inherit (worker) port;
    description = "worker endpoint, on the port this instance was given";
  };

  ix.healthChecks = {
    nginx.unit = "nginx";

    ready = {
      description = "worker answers on its readiness route";
      http = {
        inherit (worker) port;
        path = "/healthz";
      };
    };
  };
}
