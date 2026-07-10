# minestom

`packages/minestom` packages a minimal, from-scratch Minecraft server built on
[Minestom](https://minestom.net), the Java server library. Unlike a Mojang server
or a Paper/Fabric fork, Minestom ships no built-in world, loaders, mods, or EULA:
it is a library you write a server against. This package contains one example
server, `servers/hello`, built into a runnable fat jar.

- **Flake output:** `nix build .#minestom-hello-server-jar`. Also exposed at the
  package-set path `minestom.helloServerJar` (`servers/hello/package.nix:2`), so
  consumers reference it as `ix.packages.minestom.helloServerJar` /
  `pkgs.minestom.helloServerJar`.
- **Build kind:** `ix.buildGradleFatJar` (`servers/hello/default.nix:13`),
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

## Build details

`servers/hello/build.gradle.kts` (`build.gradle.kts:1`):

- `application` + `java` plugins; main class `dev.ix.minestom.Main`
  (`build.gradle.kts:21`).
- Dependencies: `net.minestom:minestom:2026.04.13-1.21.11` and
  `ch.qos.logback:logback-classic:1.5.32` (`build.gradle.kts:9`). The Minestom
  version string encodes the targeted Minecraft protocol (`1.21.11`).
- Java toolchain 25, `options.release = 25` (`build.gradle.kts:14`).
- `dependencyLocking { lockAllConfigurations() }` (`build.gradle.kts:24`) and the
  `jar` task assembles a fat jar by zipping the runtime classpath
  (`build.gradle.kts:32`).
- `settings.gradle.kts:8` resolves dependencies from an `ix.mavenRepository`
  Gradle property when present (the Nix-pinned offline repo), else Maven Central;
  `RepositoriesMode.FAIL_ON_PROJECT_REPOS` forbids per-project repos. Logging
  config is `src/main/resources/logback.xml`.

## How it is run

The jar is not a service by itself. The `services.minestom` NixOS module
(modules domain) runs a user-supplied fat jar under a JVM with no
loaders/mods/EULA. A consumer can make a runnable fleet by setting
`services.minestom.serverJar = ix.packages.minestom.helloServerJar` in `ix.nix`.
See common for the domain map.
