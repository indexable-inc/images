{index}: let
  eastWestGroup = "k8s-k3s";
in
  index.lib.mkFleet {
    nodes = {
      k3s-server = {
        groups = [eastWestGroup];
        modules = [./server.nix];
      };

      # No dependsOn: agents boot in parallel with the server and retry the
      # join until its API answers, and the server's cluster-ready check waits
      # for every agent, so `up` converges regardless of boot order.
      k3s-agent = {
        replicas = 2;
        groups = [eastWestGroup];
        modules = [./agent.nix];
      };
    };
  }
