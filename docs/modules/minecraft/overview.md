# minecraft

`modules/services/minecraft/default.nix` is the loader-agnostic Java Minecraft
server runtime. It provides the systemd unit, the JVM, the port, and the
managed-files machinery; `serverJar` and `dropinDir` are slots filled by a
loader module (fabric, paper, vanilla, ...) via module merging. Loader and mod
submodules are documented in [loaders-and-mods](loaders-and-mods.md); this page
is the core module.

Option namespace: `services.minecraft` (`default.nix:806`). This is one
component dir (`services/minecraft/`) whose loaders and mods are nested module
keys discovered as `services.minecraft.{fabric,paper,...}` and
`services.minecraft.mods.{bluemap,...}` (see [common](../common.md) for the
nesting rule).

## How a server is assembled

- A loader module (e.g. paper) sets `services.minecraft.serverJar` and
  `dropinDir`, and defaults `enable` true (`mkMinecraftLoader`,
  `lib/minecraft/loader.nix`). Set `services.minecraft.version` once and the
  loader derives the jar from `ix.artifacts.minecraft.servers."<version>-<loader>"`.
- `dropinDir` is where mod jars are symlinked: `mods` for fabric/neoforge/sponge,
  `plugins` for paper/folia/purpur/spigot (`default.nix:828`).
- All server config (server.properties, bukkit.yml, NBT) goes through
  `serverFiles`; mod config files go through `configFiles` (placed under
  `config/`) (`default.nix:9-11`).

## Public surface (selected options)

- `enable`, `version` (nullable str, single source of truth for jar + catalogs)
  (`default.nix:807`, `:809`).
- `serverJar` (package, loader-set), `dropinDir` (str, default `mods`)
  (`default.nix:823`, `:828`).
- `maxRAMPercentage` (int, 85), `javaPackage` (default Temurin JRE from
  `lib/languages/jvm-defaults.nix`), `jvmFlags` (Aikar's G1 flags)
  (`default.nix:834`, `:900`, `:902`).
- `mods` (attrs keyed by Modrinth slug), `plugins` (Bukkit-family),
  `datapacks`, `modCatalog` (defaults from `ix.artifacts.minecraft.modCatalogs`),
  `pluginCatalog`, `players` (generate whitelist.json/ops.json by UUID)
  (`default.nix:840-883`).
- `whitelist.{enable,enforce}` (`default.nix:886`).
- `properties` (server.properties), `bukkit`, `worlds`, `worldBorder`,
  `serverFiles`, `configFiles` (`default.nix:1022-1052`).
- `port` (port, 25565), `openFirewall` (bool, default false)
  (`default.nix:1058`, `:1063`).
- `rcon.{enable,port=25575,passwordFile,openFirewall,broadcastToOps}`
  (`default.nix:934`).
- `autoReload.{enable,driver,socketPath,rconPort,rconPasswordFile}` - reload
  managed mods/plugins during `nixos switch` without restarting; `driver=auto`
  uses JVM class redefinition for Fabric and PlugManX for Bukkit-family
  (`default.nix:962`).
- `yourkit` - YourKit profiler agent (`default.nix:1069`, semantics in
  `ix.languages.java.yourkit`).

## What it produces

- **Port claims** (`default.nix:1180`): `minecraft` (tcp, `port`),
  `minecraft-rcon` (when rcon enabled, `rconPort`), plus a YourKit claim.
  Firewall opens `port`/`rconPort`/yourkit ports by their `openFirewall` flags
  (`default.nix:1248`).
- **Health checks** (`default.nix:1199`): `minecraft` (unit active),
  `minecraft-status` (`mc-probe` SLP against loopback, optional
  `--motd-contains`), and `minecraft-reachable` (host `nc` to `$IX_NODE_IPV4`)
  when the firewall is open. `mc-probe` is `packages/minecraft/probe`, added to
  `environment.systemPackages` (`default.nix:1246`).
- **Extended attributes** (`default.nix:1167`): stamps `user.ix.minecraft.*`
  xattrs on the data dir, dropin dir, config dir, worlds, datapacks (via
  `ix.extendedAttributes`, see [profiles](../profiles/overview.md)).
- **systemd.services.minecraft** (`default.nix:1260`): `ix.systemdHardening` +
  `WorkingDirectory=/var/lib/minecraft`, `ExecStart` the JVM argv, `ExecReload`
  the reload command, `StateDirectory=minecraft`. `reloadTriggers`/
  `restartTriggers` are the managed roots depending on `autoReload`. `preStart`
  writes `eula=true` and runs the managed-files sync.
- **systemd.services.minecraft-world-border** (`default.nix:1287`, when
  `worldBorder.enable`): a oneshot applying the border after the server is up.
- `environment.etc."minecraft/managed-*"` expose the managed config/dropins/
  datapacks/server-files/access trees (`default.nix:1252`).

## How it is wired

Auto-discovered as `services/minecraft` with nested loader/mod keys. Server jars
and mod catalogs come from `ix.artifacts.minecraft.*` (built from
`images/games/minecraft/`). JRE defaults to the pinned Temurin major. See
[loaders-and-mods](loaders-and-mods.md).
