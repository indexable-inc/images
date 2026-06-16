# resource-monitor

`modules/services/resource-monitor/default.nix` serves a browser-accessible VM
resource monitor: a small Rust sampler writes a stats JSON on an interval and an
nginx-served Svelte single-page app renders live CPU / memory / storage usage and
a billing estimate. Both halves live in this module directory.

Option namespace: `services.resource-monitor` (`default.nix:126`).

## What it runs

- **`resource-monitor-stats-writer`** - a Rust workspace crate in
  `modules/services/resource-monitor/stats-writer/` (a root `Cargo.toml`
  workspace member, listed at `Cargo.toml:3`). `src/main.rs` loops every
  `interval-seconds`, samples `/proc/stat` CPU deltas and memory, shells out to
  `df` for storage, computes a USD/hour billing estimate, and writes the JSON
  (`stats-writer/src/main.rs:44-52`). The module selects the binary via
  `ix.cargoUnit.selectBinaryWithTests` (`default.nix:100`).
- **the Svelte site** - `modules/services/resource-monitor/site/`, built with
  `ix.buildSvelteSite` (`default.nix:85`). A generated `vm-config.json`
  (advertised capacities + billing rates) is copied in at build time
  (`default.nix:89-91`), so the UI shows the same numbers the writer bills with.

## Public surface (options)

- `enable` (`default.nix:127`).
- `port` (port, default 80) - nginx port (`default.nix:129`).
- `openFirewall` (bool, default true) (`default.nix:135`).
- `intervalSeconds` (positive int, default 1) - seconds between samples
  (`default.nix:141`).
- `runtimeDirectory` (str, default `/run/resource-monitor`) - where the stats
  JSON is written; asserted to be a managed `/run` subdirectory with safe
  segments (`default.nix:147`, asserted `default.nix:155-160`).
- Capacity/billing knobs are generated from `metricOptions`
  (`default.nix:22-65`) and merged into the option set
  (`metricOptionAttrs`, `default.nix:126`): `vcpu` (64), `memoryGiB` (256),
  `storageTiB` (1024), `cpuUsdPerVcpuMonth` (20), `memoryUsdPerGibHour` (0.005),
  `storageUsdPerTibHour` (0.0031), `marginMultiplier` (2).

## What it produces

- `ix.networking.portClaims.resource-monitor` (tcp, address `0.0.0.0`)
  (`default.nix:162`). Firewall opened on `port` when `openFirewall`
  (`default.nix:198`). No health check.
- `systemd.services.resource-monitor` (`default.nix:169`): `ix.systemdHardening`
  + `DynamicUser = true`, `RuntimeDirectory` = the `/run` subdir,
  `ExecStart` = the stats-writer binary with `--<flag> <value>` args derived from
  `statsWriterSettings` (`default.nix:105-123`).
- `services.nginx` (`default.nix:182`): a default virtual host listening on
  `port`, root at the built site, `/stats.json` rooted at `runtimeDirectory`, and
  SPA fallback `tryFiles $uri $uri/ /index.html`.

## How it is wired

Auto-discovered as `services/resource-monitor`. The stats-writer is built from
the Rust workspace; the site via the repo Svelte builder. No standalone flake
output.
