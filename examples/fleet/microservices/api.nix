{
  ix,
  name,
  nodes,
  ...
}: let
  apiPort = 8080;
  # Resolve the cache node's listener by the name it exposes it under.
  cache = ix.endpointOf nodes.cache "redis";
in {
  # Stand-in for a real service: each replica reports its own node name, so
  # requests through the gateway visibly round-robin across replicas.
  services.nginx = {
    enable = true;
    virtualHosts.api = {
      default = true;
      listen = [
        {
          addr = "0.0.0.0";
          port = apiPort;
        }
      ];
      locations."/" = {
        return = "200 '{\"service\":\"api\",\"node\":\"${name}\"}\n'";
        extraConfig = "default_type application/json;";
      };
      # A cheap dedicated readiness route for probes.
      locations."/healthz".return = "200 'ok\n'";
    };
  };

  ix.networking.expose.http = {
    port = apiPort;
    description = "api endpoint the gateway load-balances across";
  };

  ix.healthChecks = {
    nginx.unit = "nginx";

    # httpGet-style readiness probe against the dedicated route.
    ready = {
      description = "api answers on its readiness route";
      http = {
        port = apiPort;
        path = "/healthz";
      };
    };

    # tcpSocket-style probe across nodes: this replica can reach the cache.
    # During a rolling update this is what gates the next replica: a replica
    # that boots but cannot reach its dependencies stops the rollout.
    cache-reachable = {
      description = "cache is reachable from this api replica";
      tcp = {
        host = cache.host;
        port = cache.port;
      };
    };
  };
}
