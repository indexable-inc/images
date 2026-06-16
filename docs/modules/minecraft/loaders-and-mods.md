# minecraft loaders and mods

Companion to [overview](overview.md). The loader modules and mod submodules
nested under `services/minecraft/` are discovered as nested keys
(`services.minecraft.<loader>` and `services.minecraft.mods.<mod>`), because a
sibling directory with its own `default.nix` nests under its parent (see
[common](../common.md), discovery rules).

## Loaders

Each loader directory is a one-expression call to `ix.mkMinecraftLoader`
(`lib/minecraft/loader.nix`). The helper owns `services.minecraft.<name>` with
an `enable` flag and a `src` server-jar slot, assigns that jar to
`services.minecraft.serverJar`, and defaults `services.minecraft.enable` and
`dropinDir` (`lib/minecraft/loader.nix:38-60`). `src` defaults to
`ix.artifacts.minecraft.servers."<version>-<name>"`, so a caller that sets
`services.minecraft.version` rarely overrides anything per loader.

| loader | option | dropinDir | notes |
| --- | --- | --- | --- |
| vanilla | `services.minecraft.vanilla` | mods | caller passes locked Mojang jar (`vanilla/default.nix`) |
| paper | `services.minecraft.paper` | plugins | defaults `pluginCatalog` from `paperPluginCatalogs`; wires `paper-global.yml` `proxies.velocity` from `services.velocity.forwarding.secret` when present (`paper/default.nix`) |
| folia | `services.minecraft.folia` | plugins | PaperMC regionized-multithreading fork (`folia/default.nix`) |
| purpur | `services.minecraft.purpur` | plugins | Paper fork (`purpur/default.nix`) |
| spigot | `services.minecraft.spigot` | plugins | CraftBukkit fork; caller passes the jar (BuildTools has no download API) (`spigot/default.nix`) |
| neoforge | `services.minecraft.neoforge` | mods | installer-generated server jar (`neoforge/default.nix`) |
| sponge | `services.minecraft.sponge` | mods | standalone SpongeVanilla (`sponge/default.nix`) |
| fabric | `services.minecraft.fabric` | mods | defaults `javaPackage` to the shared Temurin major; loader/installer versions are baked into the pinned upstream URL (`fabric/default.nix`) |

## Mods and plugins

Each mod submodule activates only when the operator names it under
`services.minecraft.mods.<slug>` or `services.minecraft.plugins.<slug>`. They
merge config into the core module's `configFiles`/`serverFiles` and, where
needed, open ports or provision a database.

- **bluemap** (`mods/bluemap/default.nix`): 3D web map. Writes
  `bluemap/{core,webserver,storages/sql}.conf`. Port claim `bluemap` (tcp,
  default 8100) and opens it in the firewall. Optionally provisions MariaDB
  (`services.mysql`) for tile storage when `mysql = true`.
- **distant-horizons** (`mods/distant-horizons/default.nix`): server-side LOD
  generation. Generates `DistantHorizons.toml` from the attrset (defaults
  `serverSideLodGeneration = true`, `maxRenderDistance = 256`). Activated as
  `services.minecraft.mods.distanthorizons`.
- **luckperms** (`mods/luckperms/default.nix`): permissions. Optionally
  provisions MariaDB and writes `LuckPerms/config.yml` with `storage-method =
  mysql` when `mysql = true`.
- **simple-voice-chat** (`mods/simple-voice-chat/default.nix`): proximity voice.
  Writes `voicechat-server.properties`, claims and opens UDP port (default
  24454). Works as a mod or a plugin.
- **terraformgenerator** (`mods/terraformgenerator/default.nix`): Bukkit world
  generation. Binds the generator to the plugin's `worlds` (or the configured
  `level-name`) via `services.minecraft.worlds.<name>.generator =
  "TerraformGenerator"`. Plugin only.

bluemap and simple-voice-chat distinguish a mod install (`configFiles` under the
mod's config dir) from a plugin install (`serverFiles` under `plugins/`) using
the `mods.<slug>` vs `plugins.<slug>` entry the operator set.

## How they are wired

Loaders and mods are discovered as nested keys of the `minecraft` component, not
as separate top-level modules. They consume `ix.artifacts.minecraft.*` for jars
and catalogs. See [overview](overview.md) for the core option set the loaders
fill and the mods extend.
