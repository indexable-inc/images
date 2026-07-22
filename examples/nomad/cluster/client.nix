{
  ix,
  nodes,
  pkgs,
  ...
}: let
  ports = import ./ports.nix;
  whoami = import ./whoami.nix {inherit ix pkgs;};
  # Register by the server's east-west hostname, so the reference stays
  # correct regardless of which IP the server lands on.
  serverHost = nodes.nomad-server.config.ix.networking.eastWest.hostName;
in {
  imports = [./node.nix];

  services.nomad.settings = {
    client = {
      enabled = true;
      servers = [serverHost];
    };
    # raw_exec ships disabled; turning it on is the whole trick here: the
    # client is NixOS, so a job's artifact is just a store path it already
    # has, no image registry or artifact download in sight.
    plugin.raw_exec.config.enabled = true;
  };

  # Pins the job's binary (and its closure) into this client's system, which
  # is what makes the raw_exec store path valid on every node the scheduler
  # can pick.
  environment.systemPackages = [whoami];

  ix.networking.expose = {
    nomad-client-http = {
      port = ports.http;
      description = "nomad client API (alloc logs/exec are proxied here)";
    };
    whoami-http = {
      port = ports.app;
      description = "whoami allocation (static port, one per client)";
    };
  };

  # The alloc placed on this client answers; retried by `up` until the
  # scheduler has done its work.
  ix.healthChecks.whoami-http = {
    description = "whoami allocation on this client answers over http";
    http = {
      port = ports.app;
      path = "/";
    };
  };
}
