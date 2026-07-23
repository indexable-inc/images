{
  ix,
  lib,
  nodes,
  ...
}: let
  gatewayPort = 8080;
  # Discover every api VM at eval time: the `nodes` module argument carries
  # the peers wired in default.ix, so adding another api VM there grows the
  # upstream pool (and the per-VM probes below) with it.
  apiEndpoints =
    lib.mapAttrsToList (_name: node: ix.endpointOf node "http")
    (lib.filterAttrs (name: _node: name == "api" || lib.hasPrefix "api-" name) nodes);
in {
  services.nginx = {
    enable = true;

    upstreams.api.servers = lib.genAttrs' apiEndpoints (
      endpoint: lib.nameValuePair "${endpoint.host}:${toString endpoint.port}" {}
    );

    virtualHosts.gateway = {
      default = true;
      listen = [
        {
          addr = "0.0.0.0";
          port = gatewayPort;
        }
      ];
      locations."/".proxyPass = "http://api";
      locations."/healthz".return = "200 'ok\n'";
    };
  };

  ix.networking.expose.http = {
    port = gatewayPort;
    description = "public entrypoint, load-balanced across the api VMs";
  };

  ix.healthChecks =
    {
      nginx.unit = "nginx";

      # End-to-end readiness: a request through the proxy must reach a live
      # api VM, so this stays green as long as at least one is.
      proxies-to-api = {
        description = "gateway proxies requests through to an api replica";
        http.port = gatewayPort;
      };
    }
    # One probe per discovered api VM, so health output names the exact
    # upstream the gateway cannot reach.
    // lib.genAttrs' apiEndpoints (
      endpoint:
        lib.nameValuePair "upstream-${endpoint.host}" {
          description = "api VM ${endpoint.host} answers from the gateway";
          http = {
            inherit (endpoint) host port;
            path = "/healthz";
          };
        }
    );
}
