# minestom

`modules/services/minestom/default.nix` runs a Minestom server: a user-built fat
jar. Minestom is a from-scratch server library, not a Minecraft fork, so unlike
[minecraft](../minecraft/overview.md) there are no loaders, mods, or EULA. The
repo's example server is `packages/minestom/servers/hello`.

Option namespace: `services.minestom` (`default.nix:39`).

## Public surface (options)

- `enable` (`default.nix:40`).
- `serverJar` (package, required) - the fat jar to launch, built from a
  Gradle/Maven project that depends on Minestom (`default.nix:42`).
- `maxRAMPercentage` (int, default 85) (`default.nix:47`).
- `javaPackage` - default Temurin JRE from `lib/languages/jvm-defaults.nix`
  (`default.nix:52`).
- `jvmFlags` (list of str) - default `[ "-XX:+UseZGC" ]` (`default.nix:54`).
  ZGC is chosen for sub-millisecond, heap-size-independent pauses; a single G1
  pause near `MaxGCPauseMillis` would drop multiple 20-TPS ticks. Generational
  ZGC is the default mode on the JDK 24+ Temurin JRE this module targets.
- `port` (port, default 25565) (`default.nix:70`).
- `openFirewall` (bool, default true) (`default.nix:75`).
- `yourkit` - YourKit profiler agent (`default.nix:81`, type from
  `ix.languages.java.yourkit`).

## What it produces

- **Port claim** (`default.nix:94`): `minestom` (tcp, `port`) plus a YourKit
  claim when the profiler is enabled.
- **Firewall** opens `port` (and yourkit ports) when `openFirewall`
  (`default.nix:106`).
- **systemd.services.minestom** (`default.nix:109`): `ix.systemdHardening` +
  `WorkingDirectory=/var/lib/minestom`, `ExecStart` the JVM argv
  (`<java> -XX:MaxRAMPercentage=<n> <yourkit flags> <jvmFlags> -jar <serverJar>`,
  `default.nix:27-36`), `Restart=on-failure`, `StateDirectory=minestom`.

No health check.

## How it is wired

Auto-discovered as `services/minestom`. The `serverJar` is passed in by the
consumer (e.g. built from `packages/minestom/servers/<name>`); the JRE defaults
to the pinned Temurin major.
