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
        groups = [eastWestGroup];
        modules = [./worker.nix];
      };
    };
  }
