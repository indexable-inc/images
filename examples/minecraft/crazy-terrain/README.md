# Crazy Terrain

One ix fleet node running a Fabric `1.21.11` server with
[`terrain-diffusion`](https://github.com/xandergos/terrain-diffusion-mc), a
diffusion-model world generator that replaces vanilla noise with neural
terrain. The server is set up so generation runs against the absolute Java
max world height: every dimension is built at `min_y = -2032`, `height =
4064` (top `y = 2031`), and the mod's per-world `World Scale` is locked to
its `MAX_SCALE` of `6`, which puts the diffusion pipeline's `10 km` peak at
roughly `y ~2060`.

## Run

```sh
# From the index repo root.
nix run .#minecraft-crazy-terrain-up
```

## Shape

[`minecraft.nix`](minecraft.nix) is the whole image: Fabric loader,
`fabric-api`, `distanthorizons`, and `terrain-diffusion`. A `max-height`
datapack rewrites:

- `data/minecraft/dimension_type/{overworld,the_nether,the_end}.json` to the
  Java hard limit, through `services.minecraft.datapacks.<n>.dimensionTypes`
  (validated against the alignment and `min_y + height <= 2032` rules in
  [`lib/minecraft/dimension-type.nix`](../../../lib/minecraft/dimension-type.nix)).
- `data/terrain-diffusion-mc/dimension_type/terrain_diffusion.json` to the
  same bounds. The mod's default world preset points the overworld at this
  modded dimension type (not vanilla overworld), so the override has to land
  at the mod's namespaced path. It is written through the datapack's `files`
  attrset because `dimensionTypes` only emits under `data/minecraft/`.

[`world-height.nix`](world-height.nix) carries the vanilla-namespace dimension
overrides as plain data so the values stay in one place.

## World Scale

The diffusion mod stores per-world settings in a `PersistentState` blob at
`<world>/data/terrain_diffusion_world_settings.dat`. The `World Scale`
selector that the README mentions is a client-only world-creation screen; on
a dedicated server `WorldScaleManager` falls back to `DEFAULT_SCALE = 2`,
which caps peaks at roughly `y ~730` even when the dimension allows more.

`minecraft.nix` plants the dat file declaratively through
`services.minecraft.serverFiles` with `scale = 6` and `explicit_scale =
true`, so the mod loads scale 6 on first world load and skips its
no-explicit-scale fallback path. The NBT is typed through
`ix.minecraft.nbt`; the file is emitted as gzipped NBT by the `.dat` entry
in the minecraft module's `formatFor` table.

## Bad fit if

- You want vanilla terrain. The whole point of this image is the diffusion
  generator.
- You want Paper or Bukkit plugins. `terrain-diffusion` is Fabric-only; see
  [`../survival/`](../survival/) for the Paper-plus-Velocity shape.
- You are switching an already-running crazy-terrain node onto these
  settings. Dimension type bounds and the initial world scale are committed
  at world creation, so a `world/` that was generated under the old defaults
  keeps its old `min_y`, `height`, and scale. The deploy is fine on a fresh
  node; on an existing node, snapshot the VM and remove `world/` first.

## Mod build variants

The mod catalog at `packages/minecraft/catalogs/mods/1.21.11.json` pins the
**CPU** build of `terrain-diffusion`. Upstream's `v2.1.0` release ships a
sibling GPU jar; to use it, swap the URL in
`packages/minecraft/catalogs/mods/manifest.json` and regenerate with `nix run
.#update-mods -- --version 1.21.11` on a host that has a CUDA-capable GPU
exposed to the VM. The CPU build still works, it is just slower per chunk.
