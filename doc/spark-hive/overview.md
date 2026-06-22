# spark-hive

`packages/spark-hive` packages [Apache Spark](https://spark.apache.org/) 3.5,
the official complete (hadoop3 + Hive) binary distribution, self-contained for
NixOS and pinned to JDK 17. It is the Spark base the spark service module runs;
[spark-gluten](../spark-gluten/overview.md) layers the Velox backend on top.

## Why this and not nixpkgs `spark`

The nixpkgs `spark` uses the lean `bin-without-hadoop` tarball and injects an
external hadoop classpath at runtime, shipping no `hive-exec` (only
`hive-storage-api`). Apache Gluten's `HiveTableScanExecTransformer` eagerly
`classForName`s Hive's ORC and Parquet input-format classes during query
planning with no config gate, so on the lean build Gluten fails to initialize
for any query. This distribution bundles hadoop and the full Hive jar set
(`hive-exec`, `hive-common`, `hive-metastore`, `hive-serde`, `spark-hive`, ...),
so Gluten and other Hive-dependent features work with no external classpath
(`packages/spark/spark-hive/default.nix:1-12`).

## What this repo changes

`default.nix` installs the distribution and wraps every launcher
(`packages/spark/spark-hive/default.nix`):

- Source: `fetchurl` of `spark-${version}-bin-hadoop3.tgz` from
  `archive.apache.org`, pinned by `version = "3.5.5"` and an inline SRI `hash`
  (`default.nix:40-45`).
- Install (`default.nix:50-76`): move the distribution into `$out`, then
  `patchShebangs $out/bin $out/sbin` to rewrite the `#!/usr/bin/env bash`
  launchers to a store bash (systemd units run with a minimal PATH). Every
  executable launcher except the sourced `find-spark-home` is then
  `wrapProgram`ed with `--set JAVA_HOME ${jdk17_headless}`,
  `--set TZDIR ${tzdata}/share/zoneinfo`, and `bash`/`coreutils`/pyspark-Python/
  `procps` (`ps`, used by `load-spark-env.sh`) on PATH, so the units are
  self-sufficient when Spark's scripts re-exec each other.
- JDK 17 pin: `jdk17_headless` is a hard dependency arg, NOT a `jdk ? ...`
  default, because a `jdk ? jdk17_headless` arg would collide with the
  `pkgs.jdk` callPackage auto-fill (openjdk 21, which Spark 3.5 does not
  support) and silently override it. Spark 3.5 supports 8/11/17 and Gluten 1.6
  supports 8/17, so 17 is the overlap the spark service module standardizes on.
  The bin wrappers `--set JAVA_HOME`, so this is the JVM Spark runs on regardless
  of the caller's environment (`default.nix:14-17`, `31-35`).
- TZDIR: Velox (via Gluten) calls `discover_tz_dir`, which needs the IANA tz
  database; NixOS has none at the FHS `/usr/share/zoneinfo`, so the wrappers
  point `TZDIR` at the tzdata store path and executors inherit it from the
  worker's environment (`default.nix:78-80`).
- `passthru.jdk = jdk17_headless` (`default.nix:81`).
- `meta`: `mainProgram = "spark-submit"`,
  `sourceProvenance = [ binaryBytecode ]`, `platforms = [ "x86_64-linux" ]`
  (`default.nix:83-90`).

## Build and wiring

- Bump: change `version`, refresh `src.hash` with `nix-prefetch-url`, and check
  the tarball against its published `.sha512` on archive.apache.org
  (`default.nix:19-20`). No `manifest.json`/`updateScript`.
- Flake output: `nix run .#spark-hive` runs `spark-submit`. `package.nix` gates
  `packageSet` and `flake` to `x86_64-linux` (keeping `nix flake check` from
  fetching the ~400 MiB distribution on platforms nothing builds it for), while
  `overlay = true` stays unconditional and lazy, mirroring
  [spark-gluten](../spark-gluten/overview.md) and `drgn`
  (`packages/spark/spark-hive/package.nix:3-10`).
- Platform constraint: x86_64-linux only (the spark service module that consumes
  it is x86_64-linux only because the Gluten Velox bundle has no other native
  build).
