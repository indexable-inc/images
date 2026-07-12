{index}: let
  eastWestGroup = "fleet-micro";
in
  index.lib.mkFleet {
    nodes = {
      cache = {
        groups = [eastWestGroup];
        modules = [./cache.nix];
      };

      api = {
        replicas = 3;
        dependsOn = ["cache"];
        # Kubernetes RollingUpdate semantics: `up` recreates at most one api
        # replica at a time, and each must pass its health checks (including
        # "can I reach the cache?") before the next replica is touched, so a
        # bad image stops the rollout with two replicas still serving.
        updateStrategy.maxUnavailable = 1;
        groups = [eastWestGroup];
        modules = [./api.nix];
      };

      gateway = {
        dependsOn = ["api"];
        groups = [eastWestGroup];
        modules = [./gateway.nix];
      };
    };
  }
