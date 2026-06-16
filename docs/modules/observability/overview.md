# observability

`modules/services/observability/default.nix` is a self-hosted OpenTelemetry
pipeline for an ix fleet: one module, one OpenTelemetry Collector, telemetry
landing in ClickHouse and rendered in Grafana. The same module runs everywhere;
two flags pick what a node is. See the in-repo `README.md` for the flow
diagrams.

Option namespace: `services.ix-observability` (`default.nix:112`).

## Two roles

- **agent** (`agent.enable`): a local Collector on loopback that collects host
  metrics, app logs, and SDK telemetry and forwards OTLP/gRPC to a stack node.
- **stack** (`stack.enable`): the gateway Collector (binds `0.0.0.0`) plus
  ClickHouse and Grafana. Writes the ClickHouse exporter.

`enable = true` turns on both for a single node. The role-deriving defaults:
`stack.enable` defaults to `enable` (`default.nix:134-138`); `agent.enable`
defaults to `enable` (`default.nix:274`); `collector.enable` defaults to
`stack.enable || agent.enable` (`default.nix:194`); `grafana.enable` and
`query.enable` default to `stack.enable`.

## Public surface (selected options)

- `environment` (str, default `dev`) - stamped as `deployment.environment`
  (`default.nix:115`).
- `resourceAttributes` (attrs) - extra resource attrs inserted only when the
  signal does not set them (`default.nix:121`).
- `clickhouse.*`: `package` (default `pkgs.clickhouse`), `database` (`otel`),
  `host` (`127.0.0.1`), `listenAddress`, `nativePort` (9000), `httpPort` (8123),
  `ttl` (`168h` = 7 days), `openFirewall` (false) (`default.nix:140-191`).
- `collector.*`: `package` (default `opentelemetry-collector-contrib`, required
  for the ClickHouse exporter), `listenAddress`, `grpcPort` (4317), `httpPort`
  (4318), `healthPort` (13133, loopback), `openFirewall` (default
  `stack.enable`), `validateConfig` (true), `clickhouse.enable`, and
  `forward.{enable,endpoint,insecure}` for east-west forwarding
  (`default.nix:193-271`).
- `agent.*`: `endpoint` (remote collector for an agent-only node),
  `hostMetrics.enable` (true), `filelog.paths` (log globs), `journal.enable`
  (`default.nix:273-305`).
- `grafana.*`: `enable`, `port` (3000), `openFirewall` (false),
  `anonymousViewer`, `secretKeyFile` (`default.nix:307-337`).
- `query.enable` - install the `ix-observe` CLI (`default.nix:339`).

## What it produces

The `config` is an `mkMerge` of four gated blocks (`default.nix:346`):

- **stack** (`default.nix:357`): enables `services.clickhouse` with the native
  and HTTP ports, port claims `clickhouse-native`/`clickhouse-http`, firewall
  gated by `clickhouse.openFirewall`, and a `SELECT 1` health check.
- **collector** (`default.nix:403`): enables `services.opentelemetry-collector`
  with generated `settings`. Receivers: `otlp` always, plus `hostmetrics`
  (cpu/disk/filesystem/load/memory/network/paging/processes),
  `filelog/app`, and `journald` on an agent. Processors run in order:
  `memory_limiter`, `resource` (stamps `service.namespace=ix`,
  `deployment.environment`, `ix.collector.node`), `batch`. Exporters:
  `clickhouse` (on the stack; `create_schema`, `async_insert`, `lz4`,
  `otel_logs`/`otel_traces`/`otel_metrics_*` tables, `ttl`) and `otlp` (forward).
  Three pipelines (traces, metrics, logs) share the exporters. Asserts at least
  one exporter exists and a forward endpoint is set when forwarding
  (`default.nix:404-413`). Port claims `otel-grpc`/`otel-http`; health check hits
  the loopback health extension.
- **grafana** (`default.nix:570`): enables `services.grafana` with the
  ClickHouse datasource plugin, the provisioned `ClickHouse` datasource
  (uid `ix-clickhouse`), the provisioned `overview` dashboard
  (`_dashboards/overview.nix`), a generated secret key (preStart,
  `default.nix:651`), port claim `grafana`, and an `/api/health` check.
- **query** (`default.nix:675`): installs `ix-observe`, a Nushell helper
  (`default.nix:55-109`) with subcommands `logs`, `errors`, `slow-spans`,
  `trace <id>`, `sql ...` querying ClickHouse as `JSONEachRow`.

## How it is wired

Auto-discovered as `services/observability`. Dashboards live under
`_dashboards/` (underscore-prefixed, so discovery skips them). All packages are
nixpkgs (clickhouse, opentelemetry-collector-contrib, grafana); no flake output.
