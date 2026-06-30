{ index }:
let
  eastWestGroup = "ray-cluster";
in
index.lib.mkFleet {

  nodes = {
    ray-head = {
      groups = [ eastWestGroup ];
      modules = [ ./head.nix ];
    };

    ray-worker = {
      replicas = 2;
      groups = [ eastWestGroup ];
      modules = [ ./worker.nix ];
    };
  };
}
