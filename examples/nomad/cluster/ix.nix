{index}: let
  eastWestGroup = "nomad-cluster";
in
  index.lib.mkFleet {
    nodes = {
      nomad-server = {
        groups = [eastWestGroup];
        modules = [./server.nix];
      };

      # No dependsOn: clients boot in parallel with the server and retry
      # registration until it answers, and the server's job check waits for
      # allocations on every client, so `up` converges regardless of order.
      nomad-client = {
        replicas = 2;
        groups = [eastWestGroup];
        modules = [./client.nix];
      };
    };
  }
