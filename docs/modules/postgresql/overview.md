# postgresql

`modules/services/postgresql/default.nix` runs PostgreSQL 18 with defaults tuned
for an AMD EPYC Gen 5 (Zen 5) VM. It is a thin opinionated layer over the
upstream nixpkgs `services.postgresql`, which is why it claims its own `ix-`
namespace rather than clobbering the stock option tree.

Option namespace: `services.ix-postgresql` (`default.nix:20`).

## Public surface (options)

- `enable` (`default.nix:21`).
- `port` (port, default 5432) (`default.nix:23`).
- `openFirewall` (bool, default true) (`default.nix:28`).
- `dataDir` (str, default `/var/lib/postgresql/18`) (`default.nix:34`).
- `sharedBuffersMiB` (positive int, default 256) (`default.nix:39`). The single
  resize knob: it drives both the rendered `shared_buffers` and the kernel
  `vm.nr_hugepages` reservation so the two stay coherent.

## What it produces

- **Port claim + firewall.** `ix.networking.portClaims.ix-postgresql` (tcp) and
  `networking.firewall.allowedTCPPorts` gated by `openFirewall`
  (`default.nix:53-59`).
- **Health check.** `ix.healthChecks.ix-postgresql` runs `pg_isready --quiet
  --host /run/postgresql --port <port>` from the guest (`default.nix:61-76`). It
  is a real readiness probe (accepts connections), catching "starting up" /
  "rejecting connections" / crashed-but-not-failed states that
  `systemctl is-active` misses.
- **Kernel sysctls** (`default.nix:95-100`): `vm.nr_hugepages =
  (sharedBuffersMiB / 2) + 32` (sized for `shared_buffers` plus 64 MiB
  `wal_buffers` and per-connection mappings; required because `huge_pages =
  "on"` makes postmaster refuse to start without the pool), `vm.swappiness = 1`,
  `vm.dirty_background_ratio = 5`, `vm.dirty_ratio = 10`.
- **`services.postgresql`** (`default.nix:102`): `package = pkgs.postgresql_18`,
  `enableJIT = true`, and tuned `settings` written with `mkDefault` so a user
  `services.postgresql.settings.<key>` overrides them (`default.nix:110`). Notable
  defaults: `max_connections = 200`, `effective_cache_size = 768MB`,
  `wal_compression = zstd`, `io_method = worker` / `io_workers = 8` (PG 18 async
  I/O), NVMe planner costs (`random_page_cost = 1.1`,
  `effective_io_concurrency = 200`), `huge_pages = on`, `jit = on`,
  `log_min_duration_statement = 1000` (log queries over 1s).

## How it is wired

Auto-discovered as `services/postgresql`. Takes `config`, `lib`, `pkgs` (no `ix`
argument), but still uses `ix.networking.portClaims` / `ix.healthChecks`, which
are option namespaces from the platform module (see
[nix-lib](../../nix-lib/common.md)), not the `ix` specialArg. No flake package
output; the engine is `pkgs.postgresql_18`.
