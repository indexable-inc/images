_: let
  httpPort = 8080;
in {
  services.nginx = {
    enable = true;
    virtualHosts.multi-vm-hello = {
      default = true;
      listen = [
        {
          addr = "0.0.0.0";
          port = httpPort;
        }
      ];
      locations."/".return = "200 'hello from ix\n'";
    };
  };

  # One declaration opens the firewall, registers the port claim, and lets
  # workers resolve this listener with `ix.endpointOf nodes.web "http"`.
  ix.networking.expose.http = {
    port = httpPort;
    description = "hello HTTP service for the worker VMs";
  };

  ix.healthChecks = {
    nginx.unit = "nginx";

    # An httpGet-style probe: `http` desugars to a curl command that treats
    # any >= 400 status as unhealthy, and the probe binary rides the image.
    http-loopback = {
      description = "hello HTTP service answers locally";
      http.port = httpPort;
    };
  };
}
