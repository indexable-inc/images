{
  config,
  lib,
  nodes,
  ...
}: let
  ports = import ./ports.nix;
  k3s = lib.getExe' config.services.k3s.package "k3s";

  # Every VM here runs k3s under its own hostname, so the `nodes` module
  # argument (this VM plus the peers wired in default.ix) is exactly the set
  # of Kubernetes nodes the cluster must reach. Naming them (instead of
  # `--all`) makes a missing agent a failure rather than a smaller cluster
  # that happens to look Ready. Wiring another agent VM into default.ix
  # extends this automatically.
  clusterNodes =
    map (node: "node/${node.config.networking.hostName}")
    (lib.attrValues nodes);
in {
  imports = [
    ./node.nix
    ./workload.nix
  ];

  # Single server on the embedded sqlite datastore (the module default).
  # Everything not exercised by the example is disabled so the cluster is
  # exactly the scheduler, DNS, and the workload.
  services.k3s.extraFlags = [
    "--disable=traefik"
    "--disable=metrics-server"
    "--disable=servicelb"
    "--disable=local-storage"
  ];

  ix.networking.expose.k3s-api = {
    port = ports.api;
    description = "Kubernetes API server (agents join here)";
  };

  ix.healthChecks = {
    nodes-ready = {
      description = "every VM joined the cluster and reports Ready";
      command =
        [
          k3s
          "kubectl"
          "wait"
          "--for=condition=Ready"
          "--timeout=5s"
        ]
        ++ clusterNodes;
    };
    whoami-rollout = {
      description = "whoami Deployment fully rolled out";
      command = [
        k3s
        "kubectl"
        "rollout"
        "status"
        "deployment/whoami"
        "--timeout=5s"
      ];
    };
    whoami-nodeport = {
      description = "whoami Service answers on its NodePort";
      http = {
        port = ports.nodePort;
        path = "/";
      };
    };
  };
}
