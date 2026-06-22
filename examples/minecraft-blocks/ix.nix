{ index }:

# A three-node fleet that carries a Minecraft block placement from the game
# server to a 3D spatial query, the long-term data architecture in one example.
#
#   producer (Paper + plugin)  ->  log (Kafka topic)  ->  view (ClickHouse)
#                              \->  collector (OTel)   ->  view (ClickHouse)
#
# Domain facts (block placements) take the log -> view path. Server telemetry
# (the producer's own signals) takes the separate collector path. Both land in
# ONE ClickHouse: the `view` node runs the shared `services.ix-observability`
# stack (ClickHouse + collector + Grafana) and adds the `minecraft` database on
# that same server, so there is never a second ClickHouse. See README.md.
let
  # All three nodes share one east-west group so the view and producer can
  # resolve and reach the log's Kafka broker and the view's OTel collector. A
  # node outside the group has no east-west route or DNS name to its siblings.
  eastWestGroup = "minecraft-blocks";
in
index.lib.mkFleet {
  defaults = [ { ix.image.tag = "minecraft-blocks"; } ];

  nodes = {
    # The single durable, replayable source of truth.
    log = {
      groups = [ eastWestGroup ];
      modules = [ ./log.nix ];
    };

    # The view node: the shared observability stack (ClickHouse + OTel collector
    # + Grafana) plus the minecraft spatial view and Kafka ingest on the SAME
    # ClickHouse. Telemetry lands in `otel_*`; block facts land in `minecraft.*`.
    view = {
      dependsOn = [ "log" ];
      groups = [ eastWestGroup ];
      deployment.l7ProxyPorts = [ 3000 ];
      modules = [ ./view.nix ];
    };

    # The game server: emits domain facts to the log and telemetry to the
    # collector on the view node. ipv4 so the Minecraft server-list health
    # check can reach the public address.
    producer = {
      dependsOn = [
        "log"
        "view"
      ];
      groups = [ eastWestGroup ];
      deployment.ipv4 = true;
      modules = [ ./producer.nix ];
    };
  };
}
