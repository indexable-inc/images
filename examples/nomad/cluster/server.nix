_: let
  ports = import ./ports.nix;
in {
  imports = [
    ./node.nix
    ./job.nix
  ];

  services.nomad.settings.server = {
    enabled = true;
    bootstrap_expect = 1;
  };

  ix.networking.expose = {
    nomad-http = {
      port = ports.http;
      description = "nomad API and UI";
    };
    nomad-rpc = {
      port = ports.rpc;
      description = "nomad RPC (clients register here)";
    };
    nomad-serf = {
      port = ports.serf;
      description = "nomad serf gossip";
    };
    nomad-serf-udp = {
      port = ports.serf;
      protocol = "udp";
      description = "nomad serf gossip";
    };
  };
}
