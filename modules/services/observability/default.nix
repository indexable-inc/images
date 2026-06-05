{
  config,
  ix,
  lib,
  pkgs,
  ...
}:
let
  inherit (lib)
    mkEnableOption
    mkIf
    mkOption
    mkOptionDefault
    types
    ;

  cfg = config.services.ix-observability;
  datasourceUid = "ix-clickhouse";
  dashboards = pkgs.linkFarm "ix-observability-dashboards" [
    {
      name = "overview.json";
      path = import ./_dashboards/overview.nix { inherit pkgs; };
    }
  ];

  stackEnabled = cfg.stack.enable;
  agentEnabled = cfg.agent.enable;
  collectorEnabled = cfg.collector.enable;
  forwardEnabled = cfg.collector.forward.enable || (agentEnabled && !stackEnabled);
  forwardEndpoint =
    if cfg.collector.forward.endpoint != null then
      cfg.collector.forward.endpoint
    else
      cfg.agent.endpoint;
  clickhouseExporterEnabled = stackEnabled || cfg.collector.clickhouse.enable;
  exporterNames =
    lib.optional clickhouseExporterEnabled "clickhouse" ++ lib.optional forwardEnabled "otlp";
  filelogEnabled = agentEnabled && cfg.agent.filelog.paths != [ ];
  journaldEnabled = agentEnabled && cfg.agent.journal.enable;
  hostMetricsEnabled = agentEnabled && cfg.agent.hostMetrics.enable;
  listenAddress = cfg.collector.listenAddress;
  listenGrpcEndpoint = "${listenAddress}:${toString cfg.collector.grpcPort}";
  listenHttpEndpoint = "${listenAddress}:${toString cfg.collector.httpPort}";
  resourceAttributes = {
    "ix.collector.node" = config.networking.hostName;
    "service.namespace" = "ix";
    "deployment.environment" = cfg.environment;
  }
  // cfg.resourceAttributes;
  resourceProcessorAttributes = lib.mapAttrsToList (key: value: {
    inherit key value;
    action = "insert";
  }) resourceAttributes;
  clickhouseClient = "${cfg.clickhouse.package}/bin/clickhouse";
  queryTool = ix.writeNushellApplication pkgs {
    name = "ix-observe";
    runtimeInputs = [ cfg.clickhouse.package ];
    text = ''
      let clickhouse_args = [
        "client"
        "--host"
        "${cfg.clickhouse.host}"
        "--port"
        "${toString cfg.clickhouse.nativePort}"
        "--database"
        "${cfg.clickhouse.database}"
        "--format"
        "JSONEachRow"
      ]

      def run-query [sql: string] {
        ^clickhouse ...$clickhouse_args --query $sql
      }

      def "main logs" [
        --limit: int = 100
      ] {
        let row_limit = if $limit < 1 { 1 } else { $limit }
        run-query $"SELECT Timestamp, ServiceName, SeverityText, Body, TraceId, SpanId FROM otel_logs WHERE Timestamp >= now() - INTERVAL 1 HOUR ORDER BY Timestamp DESC LIMIT ($row_limit)"
      }

      def "main errors" [
        --limit: int = 100
      ] {
        let row_limit = if $limit < 1 { 1 } else { $limit }
        run-query $"SELECT Timestamp, ServiceName, SpanName, StatusCode, StatusMessage, TraceId FROM otel_traces WHERE Timestamp >= now() - INTERVAL 1 HOUR AND StatusCode = 'Error' ORDER BY Timestamp DESC LIMIT ($row_limit)"
      }

      def "main slow-spans" [
        --limit: int = 50
      ] {
        let row_limit = if $limit < 1 { 1 } else { $limit }
        run-query $"SELECT Timestamp, ServiceName, SpanName, round(Duration / 1000000.0, 2) AS duration_ms, StatusCode, TraceId FROM otel_traces WHERE Timestamp >= now() - INTERVAL 1 HOUR ORDER BY Duration DESC LIMIT ($row_limit)"
      }

      def "main trace" [trace_id: string] {
        run-query $"SELECT Timestamp, ServiceName, SpanName, SpanId, ParentSpanId, round(Duration / 1000000.0, 2) AS duration_ms, StatusCode FROM otel_traces WHERE TraceId = '($trace_id)' ORDER BY Timestamp"
      }

      def "main sql" [...query: string] {
        run-query ($query | str join " ")
      }

      def main [] {
        print "subcommands: logs, errors, slow-spans, trace, sql"
      }
    '';
    meta.description = "Query ix OpenTelemetry data in ClickHouse as JSONEachRow";
  };
in
{
  options.services.ix-observability = {
    enable = mkEnableOption "a self-hosted OpenTelemetry, ClickHouse, and Grafana stack";

    environment = mkOption {
      type = types.str;
      default = "dev";
      description = "Deployment environment stamped onto telemetry without replacing application-supplied attributes.";
    };

    resourceAttributes = mkOption {
      type = types.attrsOf (
        types.oneOf [
          types.bool
          types.float
          types.int
          types.str
        ]
      );
      default = { };
      description = "Extra resource attributes inserted by the collector when the signal does not already set them.";
    };

    stack.enable = mkOption {
      type = types.bool;
      default = cfg.enable;
      description = "Run the ClickHouse-backed collector gateway and Grafana UI on this VM.";
    };

    clickhouse = {
      package = mkOption {
        type = types.package;
        default = pkgs.clickhouse;
        defaultText = "pkgs.clickhouse";
        description = "ClickHouse package used for the server and query CLI.";
      };

      database = mkOption {
        type = types.str;
        default = "otel";
        description = "ClickHouse database where OpenTelemetry tables are created.";
      };

      host = mkOption {
        type = types.str;
        default = "127.0.0.1";
        description = "ClickHouse host used by the collector, Grafana, health checks, and query CLI.";
      };

      listenAddress = mkOption {
        type = types.str;
        defaultText = lib.literalExpression ''
          if config.services.ix-observability.clickhouse.openFirewall then "0.0.0.0" else config.services.ix-observability.clickhouse.host
        '';
        description = "Address ClickHouse binds for native and HTTP SQL.";
      };

      nativePort = mkOption {
        type = types.port;
        default = 9000;
        description = "ClickHouse native TCP port.";
      };

      httpPort = mkOption {
        type = types.port;
        default = 8123;
        description = "ClickHouse HTTP port used by Grafana when configured for HTTP.";
      };

      ttl = mkOption {
        type = types.str;
        default = "168h";
        description = "Retention TTL passed to the OpenTelemetry ClickHouse exporter.";
      };

      openFirewall = mkOption {
        type = types.bool;
        default = false;
        description = "Whether to open ClickHouse ports in the guest firewall.";
      };
    };

    collector = {
      enable = mkOption {
        type = types.bool;
        default = cfg.stack.enable || cfg.agent.enable;
        description = "Run the OpenTelemetry Collector.";
      };

      package = mkOption {
        type = types.package;
        default = pkgs.opentelemetry-collector-contrib;
        defaultText = "pkgs.opentelemetry-collector-contrib";
        description = "Collector package. The contrib build is required for the ClickHouse exporter.";
      };

      listenAddress = mkOption {
        type = types.str;
        defaultText = lib.literalExpression ''
          if config.services.ix-observability.stack.enable then "0.0.0.0" else "127.0.0.1"
        '';
        description = "Address where the collector listens for OTLP gRPC and HTTP.";
      };

      grpcPort = mkOption {
        type = types.port;
        default = 4317;
        description = "OTLP gRPC receiver port.";
      };

      httpPort = mkOption {
        type = types.port;
        default = 4318;
        description = "OTLP HTTP receiver port.";
      };

      healthPort = mkOption {
        type = types.port;
        default = 13133;
        description = "Collector health extension port on loopback.";
      };

      openFirewall = mkOption {
        type = types.bool;
        default = cfg.stack.enable;
        description = "Whether to open OTLP receiver ports in the guest firewall.";
      };

      validateConfig = mkOption {
        type = types.bool;
        default = true;
        description = "Validate the generated collector YAML during the image build.";
      };

      clickhouse.enable = mkOption {
        type = types.bool;
        default = false;
        description = "Export to the configured ClickHouse server even when stack.enable is false.";
      };

      forward = {
        enable = mkOption {
          type = types.bool;
          default = false;
          description = "Forward telemetry to another OTLP/gRPC collector.";
        };

        endpoint = mkOption {
          type = types.nullOr types.str;
          default = null;
          example = "observability:4317";
          description = "Remote OTLP/gRPC endpoint used when forwarding is enabled.";
        };

        insecure = mkOption {
          type = types.bool;
          default = true;
          description = "Use cleartext OTLP/gRPC for east-west collector forwarding.";
        };
      };
    };

    agent = {
      enable = mkOption {
        type = types.bool;
        default = cfg.enable;
        description = "Collect local host metrics and local app telemetry through the collector.";
      };

      endpoint = mkOption {
        type = types.nullOr types.str;
        default = null;
        example = "observability:4317";
        description = "Remote collector endpoint for an agent-only node.";
      };

      hostMetrics.enable = mkOption {
        type = types.bool;
        default = true;
        description = "Collect CPU, memory, disk, filesystem, process, load, paging, and network metrics.";
      };

      filelog.paths = mkOption {
        type = types.listOf types.str;
        default = [ ];
        example = [ "/var/log/my-service/*.log" ];
        description = "Log file globs collected with the OTel filelog receiver.";
      };

      journal.enable = mkOption {
        type = types.bool;
        default = false;
        description = "Collect systemd journal logs through the OTel journald receiver.";
      };
    };

    grafana = {
      enable = mkOption {
        type = types.bool;
        default = cfg.stack.enable;
        description = "Run Grafana with the ClickHouse datasource and ix dashboards provisioned.";
      };

      port = mkOption {
        type = types.port;
        default = 3000;
        description = "Grafana HTTP port.";
      };

      openFirewall = mkOption {
        type = types.bool;
        default = false;
        description = "Whether to open Grafana in the guest firewall.";
      };

      anonymousViewer = mkOption {
        type = types.bool;
        default = false;
        description = "Enable anonymous read-only dashboard access.";
      };

      secretKeyFile = mkOption {
        type = types.str;
        default = "${config.services.grafana.dataDir}/secret-key";
        description = "Runtime file holding Grafana's database encryption secret key.";
      };
    };

    query.enable = mkOption {
      type = types.bool;
      default = cfg.stack.enable;
      description = "Install the ix-observe ClickHouse query helper.";
    };
  };

  config = lib.mkMerge [
    {
      services.ix-observability = {
        # Seeded with mkOptionDefault (not a literal `default`, which would tie
        # at priority 1500 and conflict for str) so a downstream mkDefault wins.
        clickhouse.listenAddress = mkOptionDefault (
          if cfg.clickhouse.openFirewall then "0.0.0.0" else cfg.clickhouse.host
        );
        collector.listenAddress = mkOptionDefault (if cfg.stack.enable then "0.0.0.0" else "127.0.0.1");
      };
    }
    (mkIf stackEnabled {
      services.clickhouse = {
        enable = true;
        package = cfg.clickhouse.package;
        serverConfig = {
          tcp_port = cfg.clickhouse.nativePort;
          http_port = cfg.clickhouse.httpPort;
          listen_host = cfg.clickhouse.listenAddress;
        };
      };

      ix.networking.portClaims = {
        clickhouse-native = {
          protocol = "tcp";
          port = cfg.clickhouse.nativePort;
          address = cfg.clickhouse.listenAddress;
          description = "ClickHouse native SQL";
        };
        clickhouse-http = {
          protocol = "tcp";
          port = cfg.clickhouse.httpPort;
          address = cfg.clickhouse.listenAddress;
          description = "ClickHouse HTTP SQL";
        };
      };

      networking.firewall.allowedTCPPorts = lib.optionals cfg.clickhouse.openFirewall [
        cfg.clickhouse.nativePort
        cfg.clickhouse.httpPort
      ];

      ix.healthChecks.clickhouse = {
        description = "ClickHouse accepts SQL queries";
        command = [
          clickhouseClient
          "client"
          "--host"
          cfg.clickhouse.host
          "--port"
          (toString cfg.clickhouse.nativePort)
          "--query"
          "SELECT 1"
        ];
      };
    })

    (mkIf collectorEnabled {
      assertions = [
        {
          assertion = exporterNames != [ ];
          message = "services.ix-observability.collector needs at least one exporter: enable stack, collector.clickhouse, or collector.forward.";
        }
        {
          assertion = !forwardEnabled || forwardEndpoint != null;
          message = "services.ix-observability.agent.endpoint or collector.forward.endpoint must be set for an agent-only collector.";
        }
      ];

      services.opentelemetry-collector = {
        enable = true;
        package = cfg.collector.package;
        validateConfigFile = cfg.collector.validateConfig;
        settings = {
          receivers = {
            otlp.protocols = {
              grpc.endpoint = listenGrpcEndpoint;
              http.endpoint = listenHttpEndpoint;
            };
          }
          // lib.optionalAttrs hostMetricsEnabled {
            hostmetrics = {
              collection_interval = "15s";
              scrapers = {
                cpu = { };
                disk = { };
                filesystem = { };
                load = { };
                memory = { };
                network = { };
                paging = { };
                processes = { };
              };
            };
          }
          // lib.optionalAttrs filelogEnabled {
            "filelog/app" = {
              include = cfg.agent.filelog.paths;
              start_at = "beginning";
              include_file_path = true;
            };
          }
          // lib.optionalAttrs journaldEnabled {
            journald = { };
          };

          processors = {
            memory_limiter = {
              check_interval = "1s";
              limit_mib = 512;
              spike_limit_mib = 128;
            };
            batch = {
              send_batch_size = 1000;
              timeout = "10s";
            };
            resource.attributes = resourceProcessorAttributes;
          };

          exporters =
            lib.optionalAttrs clickhouseExporterEnabled {
              clickhouse = {
                endpoint = "tcp://${cfg.clickhouse.host}:${toString cfg.clickhouse.nativePort}?dial_timeout=10s";
                database = cfg.clickhouse.database;
                ttl = cfg.clickhouse.ttl;
                create_schema = true;
                async_insert = true;
                compress = "lz4";
                logs_table_name = "otel_logs";
                traces_table_name = "otel_traces";
                metrics_tables = {
                  gauge.name = "otel_metrics_gauge";
                  sum.name = "otel_metrics_sum";
                  summary.name = "otel_metrics_summary";
                  histogram.name = "otel_metrics_histogram";
                  exponential_histogram.name = "otel_metrics_exp_histogram";
                };
                retry_on_failure = {
                  enabled = true;
                  initial_interval = "5s";
                  max_interval = "30s";
                };
              };
            }
            // lib.optionalAttrs forwardEnabled {
              otlp = {
                endpoint = forwardEndpoint;
                tls.insecure = cfg.collector.forward.insecure;
              };
            };

          extensions.health_check.endpoint = "127.0.0.1:${toString cfg.collector.healthPort}";

          service = {
            extensions = [ "health_check" ];
            pipelines = {
              traces = {
                receivers = [ "otlp" ];
                processors = [
                  "memory_limiter"
                  "resource"
                  "batch"
                ];
                exporters = exporterNames;
              };
              metrics = {
                receivers = [ "otlp" ] ++ lib.optional hostMetricsEnabled "hostmetrics";
                processors = [
                  "memory_limiter"
                  "resource"
                  "batch"
                ];
                exporters = exporterNames;
              };
              logs = {
                receivers = [
                  "otlp"
                ]
                ++ lib.optional filelogEnabled "filelog/app"
                ++ lib.optional journaldEnabled "journald";
                processors = [
                  "memory_limiter"
                  "resource"
                  "batch"
                ];
                exporters = exporterNames;
              };
            };
          };
        };
      };

      ix.networking.portClaims = {
        otel-grpc = {
          protocol = "tcp";
          port = cfg.collector.grpcPort;
          address = listenAddress;
          description = "OpenTelemetry OTLP gRPC receiver";
        };
        otel-http = {
          protocol = "tcp";
          port = cfg.collector.httpPort;
          address = listenAddress;
          description = "OpenTelemetry OTLP HTTP receiver";
        };
      };

      networking.firewall.allowedTCPPorts = lib.optionals cfg.collector.openFirewall [
        cfg.collector.grpcPort
        cfg.collector.httpPort
      ];

      ix.healthChecks.otel-collector = {
        description = "OpenTelemetry Collector health endpoint";
        command = [
          (lib.getExe pkgs.curl)
          "--fail"
          "--silent"
          "--show-error"
          "http://127.0.0.1:${toString cfg.collector.healthPort}/"
        ];
      };
    })

    (mkIf cfg.grafana.enable {
      services.grafana = {
        enable = true;
        declarativePlugins = [ pkgs.grafanaPlugins.grafana-clickhouse-datasource ];
        openFirewall = cfg.grafana.openFirewall;
        settings = {
          server = {
            http_addr = "0.0.0.0";
            http_port = cfg.grafana.port;
          };
          users.allow_sign_up = false;
          auth.disable_login_form = cfg.grafana.anonymousViewer;
          "auth.anonymous" = mkIf cfg.grafana.anonymousViewer {
            enabled = true;
            org_role = "Viewer";
          };
          security = {
            disable_initial_admin_creation = cfg.grafana.anonymousViewer;
            secret_key = "$__file{${cfg.grafana.secretKeyFile}}";
          };
        };
        provision = {
          enable = true;
          datasources.settings = {
            apiVersion = 1;
            prune = true;
            datasources = [
              {
                name = "ClickHouse";
                uid = datasourceUid;
                type = "grafana-clickhouse-datasource";
                access = "proxy";
                isDefault = true;
                editable = false;
                jsonData = {
                  host = cfg.clickhouse.host;
                  port = cfg.clickhouse.nativePort;
                  protocol = "native";
                  username = "default";
                  defaultDatabase = cfg.clickhouse.database;
                  logs = {
                    defaultDatabase = cfg.clickhouse.database;
                    defaultTable = "otel_logs";
                    otelEnabled = true;
                    otelVersion = "latest";
                  };
                  traces = {
                    defaultDatabase = cfg.clickhouse.database;
                    defaultTable = "otel_traces";
                    otelEnabled = true;
                    otelVersion = "latest";
                  };
                };
              }
            ];
          };
          dashboards.settings = {
            apiVersion = 1;
            providers = [
              {
                name = "ix-observability";
                orgId = 1;
                folder = "ix Observability";
                type = "file";
                disableDeletion = false;
                updateIntervalSeconds = 60;
                allowUiUpdates = true;
                options.path = dashboards;
              }
            ];
          };
        };
      };

      ix.networking.portClaims.grafana = {
        protocol = "tcp";
        port = cfg.grafana.port;
        address = "0.0.0.0";
        description = "Grafana observability UI";
      };

      systemd.services.grafana.preStart = ''
        secret_key_file=${lib.escapeShellArg cfg.grafana.secretKeyFile}
        secret_key_dir="$(${lib.getExe' pkgs.coreutils "dirname"} "$secret_key_file")"
        ${lib.getExe' pkgs.coreutils "install"} -d -m 0700 -o grafana -g grafana "$secret_key_dir"
        if [ ! -s "$secret_key_file" ]; then
          ${lib.getExe' pkgs.coreutils "install"} -m 0600 -o grafana -g grafana /dev/null "$secret_key_file"
          ${lib.getExe pkgs.openssl} rand -base64 48 > "$secret_key_file"
          ${lib.getExe' pkgs.coreutils "chown"} grafana:grafana "$secret_key_file"
          ${lib.getExe' pkgs.coreutils "chmod"} 0600 "$secret_key_file"
        fi
      '';

      ix.healthChecks.grafana = {
        description = "Grafana health endpoint";
        command = [
          (lib.getExe pkgs.curl)
          "--fail"
          "--silent"
          "--show-error"
          "http://127.0.0.1:${toString cfg.grafana.port}/api/health"
        ];
      };
    })

    (mkIf cfg.query.enable {
      environment.systemPackages = [ queryTool ];
    })
  ];
}
