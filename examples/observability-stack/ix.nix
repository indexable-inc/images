{ index }:
let
  eastWestGroup = "observability-stack";
in
index.lib.mkFleet {

  nodes = {
    observability = {
      groups = [ eastWestGroup ];
      deployment.l7ProxyPorts = [ 3000 ];
      modules = [
        {
          services.ix-observability = {
            enable = true;
            environment = "example";
            clickhouse.openFirewall = true;
            collector.openFirewall = true;
            grafana = {
              openFirewall = true;
              anonymousViewer = true;
            };
          };
        }
      ];
    };

    app = {
      dependsOn = [ "observability" ];
      groups = [ eastWestGroup ];
      modules = [ ./app.nix ];
    };
  };
}
