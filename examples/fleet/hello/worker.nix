{
  ix,
  nodes,
  ...
}: let
  # Resolve the web node's listener by the name it exposes it under.
  web = ix.endpointOf nodes.web "http";
in {
  # A cross-node httpGet-style probe: point the sugar at the endpoint the
  # web node exposes and the platform derives the curl command (and keeps
  # curl in the image closure).
  ix.healthChecks.web-reachable = {
    description = "web service is reachable from this worker";
    http = {
      host = web.host;
      port = web.port;
    };
  };
}
