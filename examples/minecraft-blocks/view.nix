/**
  VIEW node: a materialized projection of the log for spatial queries.

  Reuses the ClickHouse that `services.ix-observability` already provisions (the
  same server that holds the `otel_*` telemetry tables); it does not stand up a
  second ClickHouse. It adds a `minecraft` database with:

  - `block_events`: the MergeTree spatial view, ordered by a Z-order (Morton)
    curve over (x, y, z) so 3D bounding-box queries scan contiguous storage.
  - `block_events_queue`: a Kafka table engine consuming the log topic.
  - `block_events_mv`: a materialized view that copies each consumed row into
    `block_events`.

  The result: a block placed on the server flows producer -> Kafka log -> this
  ClickHouse view, queryable by space. Telemetry (TPS, JVM heap) flows the
  separate OTel leg into the `otel_*` tables on the same server.
*/
{
  config,
  ix,
  lib,
  nodes,
  pkgs,
  ...
}:
let
  schema = import ./schema.nix { inherit lib; };
  packages = import ./packages.nix { inherit ix pkgs; };

  obs = config.services.ix-observability;
  clickhouseHost = obs.clickhouse.host;
  clickhousePort = obs.clickhouse.nativePort;

  # Resolve the log node's broker by the name it exposes it under.
  kafka = ix.endpointOf nodes.log "kafka";

  columnList = lib.concatStringsSep ", " schema.columnNames;

  initSql = pkgs.writeText "mc-blocks-init.sql" ''
    ${schema.createDatabaseSql};
    ${schema.createTableSql};

    CREATE TABLE IF NOT EXISTS ${schema.database}.${schema.table}_queue (
      ${schema.kafkaColumnDefs}
    )
    ENGINE = Kafka
    SETTINGS
      kafka_broker_list = '${kafka}',
      kafka_topic_list = '${schema.topic}',
      kafka_group_name = 'clickhouse-${schema.database}',
      kafka_format = 'JSONEachRow',
      kafka_num_consumers = 1;

    CREATE MATERIALIZED VIEW IF NOT EXISTS ${schema.database}.${schema.table}_mv
      TO ${schema.fullTable}
      AS SELECT ${columnList} FROM ${schema.database}.${schema.table}_queue;
  '';

  applyInit = lib.getExe (
    ix.writeNushellApplication pkgs {
      name = "mc-blocks-apply-init";
      text = ''
        let base = [ "client" "--host" "${clickhouseHost}" "--port" "${toString clickhousePort}" ]
        while (^${lib.getExe pkgs.clickhouse} ...$base --query "SELECT 1" | complete).exit_code != 0 { sleep 1sec }
        open --raw "${initSql}" | ^${lib.getExe pkgs.clickhouse} ...$base --multiquery
      '';
    }
  );
in
{
  # The shared observability stack: ONE ClickHouse holds both the `otel_*`
  # telemetry tables and the `minecraft.*` block-fact view. The collector and
  # Grafana run here too, so the server-telemetry leg has a sink on the same
  # node, never a second ClickHouse.
  services.ix-observability = {
    enable = true;
    environment = "example";
    # The ClickHouse native port stays closed: every client here runs on the
    # view node itself (the init job, the health check, and `mc-blocks`), and
    # example mode has no auth, so there is no reason to expose it east-west.
    collector.openFirewall = true;
    grafana = {
      openFirewall = true;
      anonymousViewer = true;
    };
  };

  systemd.services.mc-blocks-view-init = {
    description = "Create the minecraft block_events view, Kafka queue, and MV";
    after = [
      "clickhouse.service"
      "network-online.target"
    ];
    wants = [ "network-online.target" ];
    requires = [ "clickhouse.service" ];
    wantedBy = [ "multi-user.target" ];
    serviceConfig = {
      Type = "oneshot";
      RemainAfterExit = true;
      ExecStart = "${applyInit}";
    };
  };

  environment.systemPackages = [
    (packages.mkQueryTool {
      host = clickhouseHost;
      port = clickhousePort;
    })
  ];

  ix.healthChecks.mc-blocks-view = {
    description = "minecraft block_events view, Kafka queue, and MV exist";
    attempts = 60;
    intervalSec = 5;
    timeoutSec = 10;
    command = [
      (lib.getExe pkgs.clickhouse)
      "client"
      "--host"
      clickhouseHost
      "--port"
      (toString clickhousePort)
      "--query"
      # throwIf fails the query (non-zero exit) unless all three objects exist;
      # a bare count() would exit 0 even at count 0 and false-green the check.
      "SELECT throwIf(count() != 3, 'minecraft block_events view objects missing') FROM system.tables WHERE database = '${schema.database}' AND name IN ('${schema.table}', '${schema.table}_queue', '${schema.table}_mv')"
    ];
  };
}
