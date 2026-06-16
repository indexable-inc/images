# lib/minecraft: Minecraft Nix helpers

`lib/minecraft/` is the repo-owned Minecraft support surfaced through
`specialArgs.ix` and the flake `lib`. It covers typed NBT data, the loader
module factory, the managed-sync wrapper, and vanilla dimension-type snapshots.
`lib/default.nix` exposes `ix.minecraft.{nbt,dimensionType}`
(`lib/default.nix:273-276`), `ix.mkMinecraftLoader` (`lib/default.nix:234`),
`ix.mkMinecraftNbtFormat` (`lib/default.nix:289-291`), and
`ix.mkMinecraftSyncManaged` (`lib/default.nix:302-310`). The two Rust tools it
drives (`minecraft-nbt`, `minecraft-sync-managed`) are built via
`buildIxRustTool`.

## nbt.nix: typed tag constructors

`ix.minecraft.nbt` (`lib/minecraft/nbt.nix`) is the escape hatch for Minecraft's
narrower tag types that plain Nix scalars cannot express. Plain values
round-trip (attrset->compound, list->list, string->string, bool->byte,
int->int/long, float->double); the constructors force a specific tag:
`byte`/`short`/`int`/`long`/`float`/`double`/`string`, the typed arrays
`byteArray`/`intArray`/`longArray`, `list`, `compound`, `bool`, and `root name
value` for a named root (`lib/minecraft/nbt.nix:7-25`). Each wraps a value as
`{ __minecraftNbt = <tag>; value = ...; }` that `mkMinecraftNbtFormat` encodes.

## nbt-format.nix: a pkgs.formats-style generator

`mkMinecraftNbtFormat pkgs { format, flavor ? "uncompressed" }`
(`lib/minecraft/nbt-format.nix:6-10`) returns `{ type, generate }` matching
`pkgs.formats.*`. `format` is `snbt` (readable stringified NBT) or `nbt`
(binary); `flavor` is the binary compression: `uncompressed`/`gzip`/`zlib`
(`lib/minecraft/nbt-format.nix:12-25`). `generate name value` writes the value
to JSON then runs the `minecraft-nbt` Rust tool to emit the encoded output
(`lib/minecraft/nbt-format.nix:28-39`). Invalid `format`/`flavor` throw at eval.

## loader.nix: the loader module factory

`mkMinecraftLoader { ix, config, lib, name, dropinDir ? "mods", extraOptions ?
{}, configFragment ? _: {} }` (`lib/minecraft/loader.nix:17-25`) builds a
structurally-identical Minecraft loader module (Fabric, Paper, Vanilla, ...).
Each loader owns `services.minecraft.<name>` with an `enable` flag and a `src`
server-jar slot, and assigns the jar to `services.minecraft.serverJar`
(`lib/minecraft/loader.nix:38-60`). `src` defaults to
`ix.artifacts.minecraft.servers."${version}-${name}"`
(`lib/minecraft/loader.nix:30-35`), so a caller that sets
`services.minecraft.version` rarely overrides per loader. Reached from loader
modules via `specialArgs.ix.mkMinecraftLoader`; a loader needing more `config`
passes a `configFragment cfg` hook.

## sync-managed.nix: the sync wrapper

`mkMinecraftSyncManaged args` (`lib/default.nix:302-310`) builds the
`minecraft-sync-managed` Nushell wrapper around the Rust sync tool
(`lib/minecraft/sync-managed.nix`). It assembles the tool's argv from the mutable
data dir, managed `/etc/minecraft` roots, datapack worlds, plugman-reload and
RCON settings, and ignored plugins (`lib/minecraft/sync-managed.nix:19-50`). The
tool syncs managed files and datapacks and reconciles `whitelist.json` /
`ops.json` against the live server files by UUID. `default.nix` pre-binds the
`package` (the built `minecraft-sync-managed` binary) and `writeNushellApplication`.

## dimension-type.nix: vanilla snapshots + withBase

`ix.minecraft.dimensionType` (`lib/minecraft/dimension-type.nix:1`) exposes
`defaults` (vanilla dimension-type JSON snapshots from
`dimension-type-defaults.nix`, e.g. `minecraft:overworld`), `bases` (their
names), and `withBase name value` (`lib/minecraft/dimension-type.nix:71-83`).
`withBase` lets `services.minecraft.datapacks.<n>.dimensionTypes.<dim>` set
`base = "minecraft:overworld"` and override only the height knobs: it strips
`base`, merges the named snapshot underneath with `deepMerge.rhs`, defaults
`logical_height` to `height`, and validates the Java height constraints (16-block
alignment, the [-2032, 2031] band, the 4064 cap,
`lib/minecraft/dimension-type.nix:8-67`).
