/**
  LOG node: the durable, append-only, replayable source of truth.

  A single-node Apache Kafka broker in KRaft mode (no ZooKeeper), holding one
  topic, `minecraft.block_events`. Everything downstream (the ClickHouse view)
  derives from this topic and is rebuildable by replaying it.

  This is the runnable stand-in for the production substrate. Redpanda is the
  intended production broker (same Kafka API, so the producer and the ClickHouse
  Kafka engine are unchanged), but the broker is not packaged in this nixpkgs
  (`redpanda` now resolves only to the `rpk` client). Apache Kafka is packaged
  and Kafka-API compatible, so it is the substrate here. See the README.
*/
{
  config,
  lib,
  pkgs,
  ...
}:
let
  schema = import ./schema.nix { inherit lib; };
  brokerPort = 9092;
  controllerPort = 9093;
  kafkaBin = "${config.services.apache-kafka.package}/bin";
  # Deterministic cluster id so a rebuilt VM formats the same log dirs. KRaft
  # requires a fixed base64 UUID; this one is constant for the example.
  clusterId = "mc-blocks-kraft-000001";

  createTopic = pkgs.writeShellScript "mc-blocks-create-topic" ''
    set -eu
    until ${kafkaBin}/kafka-broker-api-versions.sh --bootstrap-server 127.0.0.1:${toString brokerPort} >/dev/null 2>/tmp/kafka-probe.err; do
      sleep 1
    done
    ${kafkaBin}/kafka-topics.sh \
      --bootstrap-server 127.0.0.1:${toString brokerPort} \
      --create --if-not-exists \
      --topic ${schema.topic} \
      --partitions 3 --replication-factor 1
  '';
in
{
  services.apache-kafka = {
    enable = true;
    formatLogDirs = true;
    inherit clusterId;
    settings = {
      # Single node wears both KRaft roles: it is its own controller quorum.
      "process.roles" = [
        "broker"
        "controller"
      ];
      "node.id" = 1;
      "controller.quorum.voters" = [ "1@127.0.0.1:${toString controllerPort}" ];
      "controller.listener.names" = [ "CONTROLLER" ];
      listeners = [
        "PLAINTEXT://0.0.0.0:${toString brokerPort}"
        "CONTROLLER://127.0.0.1:${toString controllerPort}"
      ];
      # Advertise the east-west hostname so the ClickHouse view node connects by
      # name, not localhost.
      "advertised.listeners" = [ "PLAINTEXT://${config.networking.hostName}:${toString brokerPort}" ];
      "listener.security.protocol.map" = [
        "PLAINTEXT:PLAINTEXT"
        "CONTROLLER:PLAINTEXT"
      ];
      # Single-node durability: one replica, but the topic is still the durable
      # append-only log a real cluster would replicate.
      "offsets.topic.replication.factor" = 1;
      "transaction.state.log.replication.factor" = 1;
      "transaction.state.log.min.isr" = 1;
      "log.dirs" = [ "/var/lib/apache-kafka" ];
    };
  };

  # Create the one topic once the broker is up. Idempotent, so a replaced VM
  # reconverges.
  systemd.services.mc-blocks-create-topic = {
    description = "Create the ${schema.topic} Kafka topic";
    after = [ "apache-kafka.service" ];
    requires = [ "apache-kafka.service" ];
    wantedBy = [ "multi-user.target" ];
    serviceConfig = {
      Type = "oneshot";
      RemainAfterExit = true;
      ExecStart = "${createTopic}";
    };
  };

  # One declaration: registers the claim, opens the in-guest firewall, and makes
  # the broker resolvable from sibling nodes via `ix.endpointOf nodes.log "kafka"`.
  ix.networking.expose.kafka = {
    port = brokerPort;
    address = "0.0.0.0";
    description = "Kafka broker (block_events log)";
  };

  ix.healthChecks.kafka-topic = {
    description = "Kafka broker is up and the block_events topic exists";
    attempts = 60;
    intervalSec = 5;
    timeoutSec = 10;
    command = [
      "${kafkaBin}/kafka-topics.sh"
      "--bootstrap-server"
      "127.0.0.1:${toString brokerPort}"
      "--describe"
      "--topic"
      schema.topic
    ];
  };

  environment.systemPackages = [ config.services.apache-kafka.package ];
}
