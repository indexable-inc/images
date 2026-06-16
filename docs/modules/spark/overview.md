# spark

`modules/services/spark/default.nix` runs Apache Spark 3.5 as a standalone
cluster over Tailscale, shipping the Gluten + Velox native execution engine by
default. Plain Spark runs physical operators on the JVM; Gluten offloads them to
Velox, a vectorized C++ engine, which is the source of the analytical speedups.
Enabling it is more than a classpath jar: Velox allocates off-heap and needs a
columnar shuffle manager, so the generated `spark-defaults.conf` wires the
plugin, off-heap memory, and columnar shuffle together.

Option namespace: `services.ix-spark` (`default.nix:191`).

## Topology and repo-agnostic shape

One Spark cluster. Exactly one node is `role = "master"` (runs the master daemon
and optionally a Spark Connect server); every other node is `role = "worker"`
and sets `masterAddress` to the master's tailscale IPv4. Daemons bind their
tailscale IPv4 at runtime (the `__IP__` token in argv is substituted by the
launcher), so the cluster and trust boundary are the tailnet.

Like [ray](../ray/overview.md), it declares no `ix.*` options and takes the
index lib via `_module.args.indexLib` (`default.nix:30-33`). The Spark
distribution and Gluten bundle are index-overlay packages, so an off-index
consumer passes them via `package` / `nativeEngine.package`.

## Public surface (options)

- `enable`, `role` (`master`|`worker`, default `master`), `masterAddress`
  (nullable; required on workers) (`default.nix:192-217`).
- `package` (default `pkgs.spark-hive`) - the complete Spark 3.5 distribution
  (hadoop3 + Hive) on JDK 17. Hive is mandatory: Gluten's
  `HiveTableScanExecTransformer` eagerly loads Hive input-format classes during
  planning, so the lean nixpkgs `spark` cannot initialize it (`default.nix:219`).
- `master.port` (7077), `master.webUiPort` (8080) (`default.nix:230-241`).
- `worker.enable` (true; co-locate a worker with the master),
  `worker.webUiPort` (8081), `worker.cores` (nullable), `worker.memory`
  (nullable size string) (`default.nix:243-265`).
- `connect.enable` (true; Spark Connect gRPC server on the master),
  `connect.port` (15002) (`default.nix:267-282`).
- `nativeEngine.enable` (true), `nativeEngine.package` (default
  `pkgs.spark-gluten`), `nativeEngine.offHeapSize` (`2g`, the main Velox memory
  knob) (`default.nix:284-305`).
- `openFirewall` (false) (`default.nix:307`).
- `settings` (attrs) - extra `spark-defaults.conf` entries merged over the tuned
  defaults; your keys win (`default.nix:313`).

## Key internals

- **Generated config** (`default.nix:98-119`): tuned defaults (adaptive query
  execution, Kryo serializer, pinned driver port 7078 / block-manager ports
  7079-7080) plus the native-engine block when enabled (Gluten plugin, Velox
  backend, columnar shuffle, off-heap memory, the Gluten jar on driver/executor
  classpaths, and `nativeJavaOpts`). `cfg.settings` is merged on top.
- **`nativeJavaOpts`** (`default.nix:74-81`): `--add-opens` for Arrow's JNI on
  JDK 17, `io.netty.tryReflectionSetAccessible=true` for Arrow's off-heap
  allocator, and `java.io.tmpdir` pinned to on-disk state (Gluten extracts
  ~270 MiB of native libs per JVM, which would burn RAM on the tmpfs `/tmp`).
- **Launcher** (`default.nix:135-156`): a Nushell app resolving the tailscale
  IPv4, setting `SPARK_LOCAL_IP`, substituting `__IP__` in every arg, then
  exec'ing the Spark daemon.
- **tzdata workaround** (`default.nix:349-351`): symlinks
  `/usr/share/zoneinfo` to the nixpkgs tzdata path because Velox hardcodes the
  FHS path; without it every Gluten query fails with `discover_tz_dir`.

## What it produces

- Firewall (when `openFirewall`): inter-node data ports 7078-7080 always, plus
  master RPC/web UI/Connect and worker web UI by role (`default.nix:361-376`).
- `spark` system user/group (`default.nix:353-359`).
- `systemd.services` built from `mkUnit` (`default.nix:158-178`,
  `indexLib.systemdHardening`, after `tailscaled.service`), gated by role:
  `spark-master`, co-located `spark-worker`, `spark-connect` (launched through
  `start-connect-server.sh`, not `spark-class`, so spark-submit args take
  effect), and a remote `spark-worker` on worker nodes
  (`default.nix:378-465`). No `ix.*` port claims or health checks (repo-agnostic).

## How it is wired

Auto-discovered as `services/spark`, consumed standalone via `indexLib`. Runs
`spark-hive` + `spark-gluten` from the index overlay.
