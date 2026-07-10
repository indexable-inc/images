# minestom

`packages/minestom` packages from-scratch Minecraft servers built on
[Minestom](https://minestom.net), the Java server library. Unlike a Mojang server
or a Paper/Fabric fork, Minestom ships no built-in world, loaders, mods, or EULA:
it is a library you write a server against. This package contains two example
servers, each built into a runnable fat jar:

- `servers/hello` — the minimal skeleton: one flat world, players spawn in.
- `servers/spleef` — a complete minigame: lobby, countdown, and rounds of
  spleef (dig the snow floor out from under the other players; last one
  standing wins).

Both follow the same shape:

- **Flake output:** `nix build .#minestom-hello-server-jar` /
  `.#minestom-spleef-server-jar`. Also exposed at the package-set paths
  `minestom.helloServerJar` / `minestom.spleefServerJar`
  (`servers/*/package.nix`), so consumers reference them as
  `ix.packages.minestom.<name>` / `pkgs.minestom.<name>`.
- **Build kind:** `ix.buildGradleFatJar` (`servers/*/default.nix`),
  pinned by `gradle.lockfile` and `gradle/verification-metadata.xml`.

## The hello server

`servers/hello/src/main/java/dev/ix/minestom/Main.java` is the whole server
(~30 lines):

- `MinecraftServer.init()` then create one `InstanceContainer`
  (`Main.java:11`).
- A chunk generator fills a flat world: bedrock at Y0, stone to Y36, dirt to Y39,
  grass at Y39 (`Main.java:15`).
- On `AsyncPlayerConfigurationEvent`, spawn each joining player into that
  instance with a respawn point at `(0, 42, 0)` (`Main.java:23`).
- `server.start("0.0.0.0", 25565)` (`Main.java:29`), the default Minecraft port.

## The spleef server

`servers/spleef/src/main/java/dev/ix/minestom/spleef/` is a three-class
minigame that shows the idiomatic Minestom architecture for round-based games:

- `Main.java` — `MinecraftServer.init()`, route joining players to the lobby
  via `AsyncPlayerConfigurationEvent.setSpawningInstance`.
- `Lobby.java` — a floating quartz platform in the void. A one-second
  heartbeat on the *instance-scoped* scheduler (`instance.scheduler()`) drives
  the waiting → countdown → start state machine off the live player count, and
  a boss bar shows game status. Listeners live on the *instance-scoped* event
  node (`instance.eventNode()`), not the global handler.
- `Game.java` — one round = one throwaway arena `InstanceContainer` (a snow
  disc over the void, `SNOWY_PLAINS` biome). Players are frozen on a spawn
  ring for a countdown, released with a diamond shovel, and dig snow via
  `PlayerStartDiggingEvent` (instant break). Falling below the floor
  eliminates to spectator; the last player alive wins, everyone ships back to
  the lobby, and the arena instance is unregistered — its event node,
  scheduler tasks, and chunks die with it, so rounds cannot leak state.

## Build details

`servers/*/build.gradle.kts` (spleef shown; hello differs only in pins and
main class):

- `application` + `java` plugins; main class `dev.ix.minestom.spleef.Main`
  (`build.gradle.kts:21`).
- Dependencies: `net.minestom:minestom:2026.07.01-26.1.2` and
  `ch.qos.logback:logback-classic:1.5.38` (`build.gradle.kts:9`). The Minestom
  version string encodes the targeted Minecraft protocol (`26.1.2`); hello
  still pins `2026.04.13-1.21.11`.
- Java toolchain 25, `options.release = 25` (`build.gradle.kts:14`).
- `dependencyLocking { lockAllConfigurations() }` (`build.gradle.kts:24`) and the
  `jar` task assembles a fat jar by zipping the runtime classpath
  (`build.gradle.kts:32`).
- `settings.gradle.kts:8` resolves dependencies from an `ix.mavenRepository`
  Gradle property when present (the Nix-pinned offline repo), else Maven Central;
  `RepositoriesMode.FAIL_ON_PROJECT_REPOS` forbids per-project repos. Logging
  config is `src/main/resources/logback.xml`.

## How it is run

The jars are not services by themselves. The `services.minestom` NixOS module
(modules domain) runs a user-supplied fat jar under a JVM with no
loaders/mods/EULA. A consumer can make a runnable fleet by setting
`services.minestom.serverJar = ix.packages.minestom.spleefServerJar` (or
`helloServerJar`) in `ix.nix`. See common for the domain map.

The spleef server is covered end-to-end by `checks.<system>.minestom-spleef-vm`
(`tests/minestom-spleef-vm.nix`): a NixOS VM boots the jar under
`services.minestom`, a real Minecraft server-list ping must answer with the
pinned protocol version through both renderings of the shared `mc-protocol`
crate — `mc-probe` (Python) and `mc-probe-kt` (Kotlin/FFM), both under
`packages/minecraft/minecraft` — and `mc-bot` joins as a real offline-mode
player and records the session as a ReplayMod `.mcpr`, exported as a test
artifact so a failing run leaves the client-side trace to scrub through.
