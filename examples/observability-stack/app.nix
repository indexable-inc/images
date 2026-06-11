{
  config,
  ix,
  lib,
  nodes,
  pkgs,
  ...
}:
let
  observability = {
    host = nodes.observability.config.ix.networking.eastWest.hostName;
    clickhousePort = nodes.observability.config.services.ix-observability.clickhouse.nativePort;
    database = nodes.observability.config.services.ix-observability.clickhouse.database;
  };
  # The collector's OTLP/gRPC port is an upstream module option, so build the
  # endpoint directly. Renders to "observability:<port>" in string context.
  collector = ix.endpoint {
    inherit (observability) host;
    port = nodes.observability.config.services.ix-observability.collector.grpcPort;
  };
  logDir = "/var/log/ix-observability-demo";
  logPath = "${logDir}/app.log";
  serviceName = "ix-observability-demo";
  spanName = "demo.lifecycle";
  marker = "ix-observability-demo log line";
  emitTelemetry = pkgs.writeShellScript "ix-observability-demo-emit" ''
    set -eu
    mkdir -p ${lib.escapeShellArg logDir}
    printf '%s service=%s event=started\n' ${lib.escapeShellArg marker} ${lib.escapeShellArg serviceName} >> ${lib.escapeShellArg logPath}
    ${lib.getExe pkgs.otel-cli} span \
      --service ${lib.escapeShellArg serviceName} \
      --name ${lib.escapeShellArg spanName} \
      --endpoint 127.0.0.1:${toString config.services.ix-observability.collector.grpcPort} \
      --protocol grpc \
      --insecure \
      --fail \
      --attrs ix.example=observability-stack,ix.node=${lib.escapeShellArg config.networking.hostName}
  '';
  checkTelemetry = pkgs.writeShellScript "ix-observability-demo-check" ''
    set -eu
    trace_count="$(${lib.getExe pkgs.clickhouse} client --host ${lib.escapeShellArg observability.host} --port ${toString observability.clickhousePort} --database ${lib.escapeShellArg observability.database} --query "SELECT count() FROM otel_traces WHERE ServiceName = '${serviceName}' AND SpanName = '${spanName}' AND Timestamp >= now() - INTERVAL 1 DAY")"
    log_count="$(${lib.getExe pkgs.clickhouse} client --host ${lib.escapeShellArg observability.host} --port ${toString observability.clickhousePort} --database ${lib.escapeShellArg observability.database} --query "SELECT count() FROM otel_logs WHERE Body LIKE '%${marker}%' AND Timestamp >= now() - INTERVAL 1 DAY")"
    test "$trace_count" -gt 0
    test "$log_count" -gt 0
  '';
in
{
  services.ix-observability = {
    stack.enable = false;
    agent = {
      enable = true;
      endpoint = "${collector}";
      filelog.paths = [ logPath ];
    };
    environment = "example";
    resourceAttributes."ix.example" = "observability-stack";
  };

  systemd.tmpfiles.rules = [
    "d ${logDir} 0755 root root -"
  ];

  environment.systemPackages = [
    pkgs.clickhouse
    pkgs.otel-cli
  ];

  systemd.services.ix-observability-demo = {
    description = "Emit demo OpenTelemetry signals";
    after = [
      "network-online.target"
      "opentelemetry-collector.service"
    ];
    wants = [ "network-online.target" ];
    wantedBy = [ "multi-user.target" ];
    serviceConfig = {
      Type = "oneshot";
      RemainAfterExit = true;
      ExecStart = "${emitTelemetry}";
    };
  };

  ix.healthChecks = {
    observability-demo = {
      description = "demo telemetry emitter ran";
      unit = "ix-observability-demo";
    };

    observability-ingested = {
      description = "demo span and log reached ClickHouse";
      attempts = 60;
      intervalSec = 5;
      timeoutSec = 10;
      command = [ "${checkTelemetry}" ];
    };
  };
}
