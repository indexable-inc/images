{pkgs}: let
  dashboard = import ./lib.nix {inherit pkgs;};
in
  dashboard.json.generate "ix-observability-overview.json" {
    title = "ix Observability";
    uid = "ix-observability";
    tags = [
      "ix"
      "opentelemetry"
      "clickhouse"
    ];
    timezone = "browser";
    schemaVersion = 39;
    version = 1;
    refresh = "30s";
    time = {
      from = "now-1h";
      to = "now";
    };
    panels = dashboard.layoutRows [
      {
        height = 4;
        panels = [
          (dashboard.clickhouseStatPanel {
            id = 1;
            title = "Spans (15m)";
            rawSql = ''
              SELECT count() AS value
              FROM otel_traces
              WHERE Timestamp >= now() - INTERVAL 15 MINUTE
            '';
            thresholds = [
              (dashboard.thresholdStep {color = "red";})
              (dashboard.thresholdStep {
                color = "green";
                value = 1;
              })
            ];
          })
          (dashboard.clickhouseStatPanel {
            id = 2;
            title = "Error Spans (15m)";
            rawSql = ''
              SELECT count() AS value
              FROM otel_traces
              WHERE Timestamp >= now() - INTERVAL 15 MINUTE
                AND StatusCode = 'Error'
            '';
            thresholds = [
              (dashboard.thresholdStep {color = "green";})
              (dashboard.thresholdStep {
                color = "red";
                value = 1;
              })
            ];
          })
          (dashboard.clickhouseStatPanel {
            id = 3;
            title = "Logs (15m)";
            rawSql = ''
              SELECT count() AS value
              FROM otel_logs
              WHERE Timestamp >= now() - INTERVAL 15 MINUTE
            '';
          })
          (dashboard.clickhouseStatPanel {
            id = 4;
            title = "Services (1h)";
            rawSql = ''
              SELECT countDistinct(ServiceName) AS value
              FROM otel_traces
              WHERE Timestamp >= now() - INTERVAL 1 HOUR
            '';
          })
        ];
      }
      {
        height = 8;
        panels = [
          (dashboard.span 2 (
            dashboard.clickhouseTimeseriesPanel {
              id = 5;
              title = "Span Throughput";
              rawSql = ''
                SELECT
                  toStartOfInterval(Timestamp, INTERVAL 1 MINUTE) AS time,
                  ServiceName AS metric,
                  count() AS spans
                FROM otel_traces
                WHERE $__timeFilter(Timestamp)
                GROUP BY time, metric
                ORDER BY time, metric
              '';
            }
          ))
          (dashboard.span 2 (
            dashboard.clickhouseTimeseriesPanel {
              id = 6;
              title = "Log Volume";
              rawSql = ''
                SELECT
                  toStartOfInterval(Timestamp, INTERVAL 1 MINUTE) AS time,
                  if(empty(SeverityText), 'unknown', SeverityText) AS metric,
                  count() AS logs
                FROM otel_logs
                WHERE $__timeFilter(Timestamp)
                GROUP BY time, metric
                ORDER BY time, metric
              '';
            }
          ))
        ];
      }
      {
        height = 9;
        panels = [
          (dashboard.span 2 (
            dashboard.clickhouseTablePanel {
              id = 7;
              title = "Slow Spans";
              format = 1;
              rawSql = ''
                SELECT
                  Timestamp,
                  ServiceName,
                  SpanName,
                  round(Duration / 1000000.0, 2) AS duration_ms,
                  StatusCode,
                  TraceId
                FROM otel_traces
                WHERE $__timeFilter(Timestamp)
                ORDER BY Duration DESC
                LIMIT 50
              '';
            }
          ))
          (dashboard.span 2 (
            dashboard.clickhouseLogsPanel {
              id = 8;
              title = "Recent Logs";
              rawSql = ''
                SELECT
                  Timestamp AS "timestamp",
                  concat('[', ServiceName, '] ', Body) AS "body",
                  SeverityText AS "level",
                  TraceId AS "traceID",
                  SpanId AS "spanID"
                FROM otel_logs
                WHERE $__timeFilter(Timestamp)
                ORDER BY Timestamp DESC
                LIMIT 200
              '';
            }
          ))
        ];
      }
    ];
  }
