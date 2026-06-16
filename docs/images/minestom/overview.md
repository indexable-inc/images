# minestom

`images/games/minestom` is a Minestom hello-world server image. Minestom is a
from-scratch server library (not a Minecraft loader), so there are no loaders,
mods, or EULA: the image runs a user-built fat jar. Flake output `.#minestom`.

## What it builds

`images/games/minestom/default.nix` (13 lines):

```nix
ix.image.name = "minestom-hello";          # default.nix:7
services.minestom = {
  enable = true;
  serverJar = ix.packages.minestom.helloServerJar;   # default.nix:9-12
};
```

The OCI image name is `minestom-hello` while the flake output is `minestom`. The
`serverJar` is the repo-built demo fat jar `ix.packages.minestom.helloServerJar`.

## Composed module: `services.minestom`

Defined in `modules/services/minestom/default.nix`. Runs the fat jar under a JVM
(`:1-4`). Key surface:

- `enable` (`:40`), `serverJar` (the fat jar to launch, built from a
  Gradle/Maven project depending on Minestom, `:42-45`).
- `port` (default 25565, `:70-73`), `openFirewall` (default true, `:75-79`).
- `maxRAMPercentage` (default 85, `:47-50`), `javaPackage` (Temurin JRE,
  `:52`), `jvmFlags` (default `-XX:+UseZGC` for sub-millisecond pauses, chosen
  over the Java-Minecraft Aikar G1GC flags because a long pause drops 20 TPS
  ticks, `:54-68`).
- `yourkit` (optional YourKit profiler agent; `ix.languages.java.yourkit`,
  `:81-90`).

Runtime wiring (`:93-122`): claims TCP `port` (`minestom`) plus the YourKit port
when enabled, opens the firewall for `openFirewall`, and runs
`systemd.services.minestom` (hardened, `StateDirectory = minestom`) execing
`java -XX:MaxRAMPercentage=85 <yourkit flags> <jvmFlags> -jar <serverJar>`
(`:27-36,109-121`).

## Build

```
nix build .#minestom
```

Minestom listens on TCP 25565 by default (opened by default). The base platform
also opens its standard ix-console/ix-agent ports; see [common](../common.md).

## Notes

- To run your own server, set `services.minestom.serverJar` to your fat jar
  (built e.g. via `ix.buildGradleFatJar`, `lib/build/gradle-fat-jar.nix`).
- The `services.minestom` module's YourKit and JVM-flag wiring is pinned by eval
  tests (`tests/default.nix:2159-2202,4413-4445`), e.g. `-agentpath:` injection
  and the default YourKit port 10001.
