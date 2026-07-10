{index}: let
  eastWestGroup = "fleet-hello";
in
  index.lib.mkFleet {
    nodes = {
      web = {
        groups = [eastWestGroup];
        modules = [./web.nix];
      };

      worker = {
        replicas = 3;
        dependsOn = ["web"];
        # Roll workers one at a time: `up` recreates at most one replica
        # concurrently, and each must pass its health checks before the
        # next one is touched.
        updateStrategy.maxUnavailable = 1;
        groups = [eastWestGroup];
        modules = [./worker.nix];
      };
    };
  }
