# Factions Server

Standalone consumer example for a Paper factions server on ix.

It defines one Paper `26.1.2` server with a curated plugin set, a `12000` block
world border, a 4064-block max-height datapack, BlueMap on TCP `8100`, Simple
Voice Chat on UDP `24454`, and local-only RCON for managed reloads.

Customize real player UUIDs and spawn/claim policy before using it with real
players.

## Run

```sh
# From the index repo root.
nix run .#factions-up
```

## Shape

- [`minecraft.nix`](minecraft.nix) wires the Minecraft service.
- [`plugins.nix`](plugins.nix) selects factions, economy, audit, map, voice, and
  scripting plugins from the generated catalog.
- [`world.nix`](world.nix) owns the seed and border constants.
- [`world-height.nix`](world-height.nix) contains the generated datapack.
- [`bukkit.nix`](bukkit.nix), [`paper.nix`](paper.nix), and
  [`spigot.nix`](spigot.nix) hold loader config files.

The world border is applied after startup through local RCON. RCON stays off the
firewall by default; ix uses it to apply the border and reload managed Paper
plugins during a switch.
