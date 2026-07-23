<p align="center">
  <picture>
    <source media="(prefers-color-scheme: dark)" srcset="assets/hero-dark.svg">
    <img src="assets/hero.svg" width="720" alt="an app VM's collector agent forwards spans and tailed logs over OTLP to an observability VM running a collector, ClickHouse, and Grafana">
  </picture>
</p>

# Observability stack

Where do spans and logs go when your app runs in a VM you created five
minutes ago? This example self-hosts the whole pipeline: an `observability`
VM runs an OpenTelemetry collector, ClickHouse, and Grafana, and an `app`
VM exercises both instrumentation paths by sending a span through its local
collector agent and writing a log line the agent tails. A health check
holds the app unhealthy until both records actually land in ClickHouse.

For how the telemetry pipeline works end to end, with diagrams, see the
[module README](../../../modules/services/observability/README.md).

## Run

```sh
ix apply .#observability .#app
```

The stack first, so the app's telemetry has somewhere to land.

Grafana is on port `3000` through the example's L7 proxy. The
ClickHouse-backed query helper is available inside the observability VM:

```sh
ix shell observability -- ix-observe logs --limit 20
ix shell observability -- ix-observe slow-spans
```

Get the source with `git clone https://github.com/indexable-inc/index`
and run it from `examples/observability/stack`.

## Shape

- `observability` runs
  [`services.ix-observability`](../../../modules/services/observability/).
- `app` enables only the local collector agent and forwards OTLP to
  `observability:4317`.
- [`app.nix`](app.nix) proves both instrumentation paths: direct OTLP spans
  and file-tailed logs.

## Swap in your service

Keep the observability VM, then add this to an application VM:

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
