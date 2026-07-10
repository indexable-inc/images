/**
PRODUCER node: a Paper Minecraft server emitting domain facts, plus the
transport that ships them to the log.

Two legs run side by side here, and keeping them separate is the point:

- DOMAIN FACTS: the BlockEvents plugin writes one JSON Lines record per
  placement to a file; a tail-to-Kafka service produces those records to the
  `minecraft.block_events` topic on the log node. This is the log -> view
  leg. Block placements are facts to aggregate and range-query, so they go
  here, not through the telemetry collector.

- SERVER TELEMETRY: the OTel agent forwards the server's own signals (logs,
  host metrics, and any OTLP the server emits about TPS or JVM heap) to the
  observability collector, landing in the `otel_*` ClickHouse tables. This is
  the separate collector leg.
*/
{
  ix,
  lib,
  nodes,
  pkgs,
  ...
}: let
  schema = import ./schema.nix {inherit lib;};
  packages = import ./packages.nix {inherit ix pkgs;};

  blockLog = "/var/lib/minecraft/block-events.jsonl";
  kafkaBin = "${pkgs.apacheKafka}/bin";

  # The log node declares the broker via `ix.networking.expose.kafka`, so we
  # resolve its reachable `host:port` by name instead of reaching into its
  # option tree. `kafka` renders to "log:9092" in string context.
  kafka = ix.endpointOf nodes.log "kafka";

  # The view node runs the shared observability stack, so it is also the
  # telemetry sink. One ClickHouse, two legs. The collector port is an upstream
  # module option (not an `expose`), so build the endpoint directly.
  collector = ix.endpoint {
    host = nodes.view.config.ix.networking.eastWest.hostName;
    port = nodes.view.config.services.ix-observability.collector.grpcPort;
  };

  # Transport leg: follow the plugin's append-only file and produce each new
  # line to the topic. `tail -F -n +1` replays the whole file on first start
  # then streams; on a restart it re-sends from the top. That re-send is
  # deliberately left in place and is harmless: the transport is at-least-once,
  # and the ClickHouse view is a ReplacingMergeTree keyed on the placement
  # identity (schema.nix), so a replayed record collapses back to one row. The
  # honest pattern is at-least-once transport feeding an idempotent view, which
  # is effectively-once end to end; the log, not the transport, is the source of
  # truth, and duplicate delivery never corrupts a count.
  shipToKafka = lib.getExe (
    ix.writeNushellApplication pkgs {
      name = "mc-blocks-ship";
      text = ''
        touch "${blockLog}"
        while (^${kafkaBin}/kafka-broker-api-versions.sh --bootstrap-server "${kafka}" | complete).exit_code != 0 {
          sleep 2sec
        }
        # Stream the append-only file into the topic (at-least-once; the view is
        # idempotent). nushell keeps the pipe as the long-running service body.
        ^${pkgs.coreutils}/bin/tail -F -n +1 "${blockLog}" | ^${kafkaBin}/kafka-console-producer.sh --bootstrap-server "${kafka}" --topic "${schema.topic}"
      '';
    }
  );
in {
  # The Minecraft server with the custom block-events plugin.
  services.minecraft = {
    enable = true;
    version = "26.1.2";
    paper.enable = true;
    openFirewall = true;

    properties = {
      motd = "ix block-events demo";
      gamemode = "creative";
      level-name = "blocks";
      online-mode = false;
      spawn-protection = 0;
    };

    plugins.block-events = {
      enable = true;
      src = packages.plugin;
      pluginName = "BlockEvents";
    };

    # Tell the plugin where to append, so the transport service and the plugin
    # agree on the file. `serverFiles` lands managed config under the server
    # root without the server writing back.
    serverFiles."plugins/BlockEvents/config.yml" = {
      logPath = blockLog;
    };
  };

  # DOMAIN-FACT transport: file -> Kafka topic.
  systemd.services.mc-blocks-ship = {
    description = "Ship block-event records to the Kafka log topic";
    after = [
      "network-online.target"
      "minecraft.service"
    ];
    wants = ["network-online.target"];
    wantedBy = ["multi-user.target"];
    serviceConfig = {
      ExecStart = "${shipToKafka}";
      Restart = "always";
      RestartSec = 5;
    };
  };

  # SERVER-TELEMETRY leg: forward the server's own signals to the collector.
  # This is deliberately the OTel path, separate from the domain-fact path
  # above, so the diagram's two legs are real in the running fleet.
  #
  # Collect the server console from the systemd journal, not by tailing
  # `/var/lib/minecraft/logs/latest.log`. The minecraft service runs Type=simple
  # with no StandardOutput override, so its stdout (the full Paper console) lands
  # in the journal, while the log file lives under the server's private state
  # directory (UMask 0077) that the DynamicUser collector cannot read. The OTel
  # journald receiver runs `journalctl`, and the upstream collector unit already
  # joins the `systemd-journal` group, so this path is actually readable.
  services.ix-observability = {
    stack.enable = false;
    agent = {
      enable = true;
      endpoint = "${collector}";
      journal.enable = true;
    };
    environment = "example";
    resourceAttributes."ix.app" = "minecraft-blocks";
  };

  environment.systemPackages = [pkgs.apacheKafka];

  # Prove the producer leg is live: the plugin jar is installed and the
  # transport service is running.
  ix.healthChecks.mc-blocks-producer = {
    description = "block-events plugin installed and Kafka shipper active";
    unit = "mc-blocks-ship";
  };
}
