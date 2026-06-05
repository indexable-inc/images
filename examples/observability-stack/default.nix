{ index }:
let
  eastWestGroup = "observability-stack";
in
index.lib.mkFleet {
  defaults = [ { ix.image.tag = "observability-stack"; } ];

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
            # Exercise the RFC 0004 bus archive leg: the collector mirrors the
            # logs pipeline to S3 as OTLP/JSON for the source-otlp consumer.
            collector.archive = {
              enable = true;
              endpoint = "http://127.0.0.1:9010";
            };
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
