{
  ix,
  lib,
  pkgs,
  nodes,
  ...
}:
let
  package = import ./package.nix { inherit ix lib pkgs; };
  headHost = nodes.ray-head.config.ix.networking.eastWest.hostName;

  # Every fleet node here runs Ray, so the node count is the cluster size the
  # health check should wait for. Bumping `ray-worker.replicas` in default.nix
  # raises this automatically.
  expectedNodes = builtins.length (builtins.attrNames nodes);

  gcsPort = 6379;
  clientPort = 10001;
in
{
  imports = [
    (import ./cluster-node.nix {
      inherit ix lib pkgs;
      role = "head";
      extraStartArgs = [
        "--head"
        "--port"
        (toString gcsPort)
        "--ray-client-server-port"
        (toString clientPort)
        # Core Ray only; the dashboard needs the `ray[default]` extras. The
        # README notes what to add to turn it on.
        "--include-dashboard"
        "false"
      ];
    })
  ];

  # GCS bootstrap and the Ray Client server are head-only listeners; the shared
  # node-manager, object-manager, and worker ports are opened in cluster-node.
  networking.firewall.allowedTCPPorts = [
    gcsPort
    clientPort
  ];

  ix.networking.portClaims = {
    ray-gcs = {
      protocol = "tcp";
      port = gcsPort;
      description = "Ray GCS (cluster bootstrap)";
    };
    ray-client = {
      protocol = "tcp";
      port = clientPort;
      description = "Ray Client server";
    };
  };

  # The driver attaches to the local head over GCS, fails until every worker
  # has joined, then fans tasks across the cluster. This is both the liveness
  # gate and the worked example: its log shows the per-node task placement.
  ix.healthChecks.ray-cluster = {
    description = "Ray cluster reached ${toString expectedNodes} nodes and ran distributed tasks";
    command = [
      (lib.getExe package)
      "--address"
      "${headHost}:${toString gcsPort}"
      "--min-nodes"
      (toString expectedNodes)
    ];
  };
}
