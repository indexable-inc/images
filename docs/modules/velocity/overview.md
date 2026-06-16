# velocity

`modules/services/velocity/default.nix` runs the Velocity Minecraft proxy
(papermc.io/software/velocity). It fronts backend Java servers, manages a set of
plugins, and renders `velocity.toml` plus arbitrary managed config files. It is
the host for the [geyser](../geyser/overview.md) and
[floodgate](../floodgate/overview.md) plugins.

Option namespace: `services.velocity` (`default.nix:238`).

## What it runs

The Velocity proxy jar. Default `package` is
`ix.artifacts.minecraft.velocityServers."3.4.0-SNAPSHOT".src` (`default.nix:241`),
a locked artifact from the generated catalog. The JVM is the pinned Temurin JRE
(`default.nix:248`); the argv is `<java> -XX:MaxRAMPercentage=<n> <yourkit>
<jvmFlags> -jar <package>` (`default.nix:226-235`).

## Public surface (selected options)

- `enable`, `package`, `javaPackage`, `maxRAMPercentage` (75), `jvmFlags`,
  `yourkit` (`default.nix:239-291`).
- `address` (`0.0.0.0`), `port` (25565), `openFirewall` (true)
  (`default.nix:264-280`).
- `motd`, `health.motdContains` (substrings the rendered MOTD must contain for
  the `velocity-status` check) (`default.nix:293`, `:299`).
- `onlineMode` (true), `forceKeyAuthentication`, `forwarding.{mode,secret,secretFile}`
  (`mode` default `modern`; secret lands in the store, secretFile is a runtime
  file) (`default.nix:320-361`).
- `servers` (backend name -> address), `try`, `forcedHosts` (`default.nix:398-415`).
- `advanced.*` (compression, timeouts, rate limits, reuse-port, ...)
  (`default.nix:417`).
- `query.{enable,port=25565,openFirewall,map,showPlugins}` - GameSpy 4 query
  listener (UDP) (`default.nix:539`).
- `plugins` (attrs of `{enable,src,fileName}`; empty `{}` resolves a catalog
  plugin by slug, `src` installs a local jar), `pluginCatalog` (default
  `ix.artifacts.minecraft.velocityPluginCatalog`) (`default.nix:567`, `:573`).
- `configFiles` (managed files relative to the data dir; `.json/.properties/
  .toml/.yaml/.yml`), `settings` (raw `velocity.toml` merged over typed options)
  (`default.nix:588`, `:594`).

## Key internals

- **Managed config and plugins** (`default.nix:153-206`): `velocity.toml` is
  rendered from typed options merged with `settings`, plus every `configFiles`
  entry, into a `runCommand` symlink farm; plugins are resolved from `src` or the
  catalog into another farm. `preStart` (`default.nix:722`) reconciles the
  managed plugin manifest, installs config files, and either copies the
  forwarding secret or generates one with `openssl rand` if none is set
  (`installForwardingSecret`, `default.nix:213-224`).
- **Assertions** (`default.nix:602-623`): cannot set both `forwarding.secret`
  and `secretFile`; `configFiles` cannot manage `velocity.toml`; reject unsafe
  relative paths/names and duplicate plugin file names (via `ix.relativePath`,
  `ix.lists`).

## What it produces

- **Port claims** (`default.nix:625`): `velocity` (tcp), `velocity-query` (udp,
  when query enabled), yourkit. Firewall opens the client port, query port, and
  yourkit ports by their flags (`default.nix:644-648`).
- **Health checks** (`default.nix:650`): `velocity` (unit active),
  `velocity-status` (`mc-probe` SLP against the bind address, loopback for
  wildcard binds, optional `--motd-contains`), and `velocity-reachable` (host
  `nc`) when the firewall is open. `mc-probe` (`packages/minecraft/probe`) is on
  PATH (`default.nix:697`).
- A `velocity` system user/group with home `/var/lib/velocity`
  (`default.nix:704`).
- **systemd.services.velocity** (`default.nix:712`): runs as that user,
  `ix.systemdHardening`, `WorkingDirectory`/`StateDirectory = velocity`,
  `ExecStart` the JVM argv. `restartTriggers` track the managed config/plugins
  and forwarding secret.
- `environment.etc."velocity/managed-{config,plugins}"` (`default.nix:699`).

## Relationship to Geyser / Floodgate

[geyser](../geyser/overview.md) and [floodgate](../floodgate/overview.md) do not
run their own units: each composes onto `services.velocity` by adding a plugin
entry (with `src` and `fileName`) and managed config files, defaulting
`services.velocity.enable` true. Paper backends behind Velocity pick up the
shared forwarding secret via the paper loader (see
[minecraft loaders](../minecraft/loaders-and-mods.md)).

## How it is wired

Auto-discovered as `services/velocity`. Jar and plugin catalog come from
`ix.artifacts.minecraft.*`.
