# Observability Stack

An ix fleet with a self-hosted OpenTelemetry collector, ClickHouse, and
Grafana. The app node sends a span through its local collector, writes a log
line that the collector tails, and checks that both records land in ClickHouse.

For how the telemetry pipeline works end to end, with diagrams, see the
[module README](../../modules/services/observability/README.md).

## Run

```sh
nix run .#observability-stack-up
```

Grafana is on port `3000` through the example's L7 proxy. The ClickHouse-backed
query helper is available inside the observability VM:

```sh
ix shell observability -- ix-observe logs --limit 20
ix shell observability -- ix-observe slow-spans
```

## Shape

- `observability` runs [`services.ix-observability`](../../modules/services/observability/).
- `app` enables only the local collector agent and forwards OTLP to
  `observability:4317`.
- [`app.nix`](app.nix) proves both instrumentation paths: direct OTLP spans and
  file-tailed logs.

## Swap In Your Service

Keep the observability node, then add this to an application VM:

```nix
{
  services.ix-observability = {
    stack.enable = false;
    agent = {
      enable = true;
      endpoint = "observability:4317";
      filelog.paths = [ "/var/log/my-service/*.log" ];
    };
    resourceAttributes."ix.app" = "my-service";
  };
}
```

Use normal OpenTelemetry SDK settings in the app, pointed at
`127.0.0.1:4317`. The local collector handles batching, resource labels, and
the remote write to ClickHouse.
