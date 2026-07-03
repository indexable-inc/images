{ix, ...}: let
  worldHeight = import ./world-height.nix;

  # The diffusion mod's `World Scale` GUI is client-only and saves into a
  # `data/terrain_diffusion_world_settings.dat` PersistentState file per
  # world. A dedicated server never runs that GUI, so `WorldScaleManager`
  # falls back to `DEFAULT_SCALE = 2`, which caps generated peaks at roughly
  # y ~730. Planting the dat ourselves with `explicit_scale = true` shortcuts
  # the fallback and locks in scale 6 (≈y ~2060 at the upstream pipeline's
  # 10 km ceiling): the mod loads our value verbatim on first world load.
  worldScale = 6;
in {
  services.minecraft = {
    enable = true;
    version = "1.21.11";
    fabric.enable = true;

    properties = {
      motd = "ix Crazy Terrain";
      max-players = 20;
      online-mode = false;
      # The terrain pass is expensive, but it is also the whole point of the
      # image, so spend chunks on it. Diffusion is bottlenecked per-chunk;
      # large view radii are fine, the work is amortized across players.
      view-distance = 32;
      simulation-distance = 12;
    };

    mods = {
      fabric-api = {};
      distanthorizons = {
        serverSideLodGeneration = true;
        maxRenderDistance = 32;
      };
      # Diffusion-model world generator. The catalog jar at
      # packages/minecraft/catalogs/mods/1.21.11.json is the CPU build; the same
      # upstream release ships a GPU variant. Swap the manifest URL and
      # regenerate with `nix run .#update-mods -- --version 1.21.11` on a host
      # with a CUDA-capable GPU.
      terrain-diffusion = {};
    };

    # Repo-own the mod's config file so the defaults are explicit and the
    # file is symlinked from the store rather than written by the mod on
    # first launch. The CPU build's `auto` device already falls back to CPU
    # on Linux; spell `cpu` out so a future GPU build does not silently flip
    # behaviour after a server image rebuild.
    configFiles."terrain-diffusion-mc.properties" = {
      inference = {
        device = "cpu";
        offload_models = true;
      };
      validate_model = true;
      explorer = {
        port = 19801;
      };
      spawn_search = {
        initial_size = 16;
        max_size = 128;
      };
    };

    datapacks."max-height" = {
      inherit (worldHeight) dimensionTypes pack;
      # `dimensionTypes` only writes under `data/minecraft/dimension_type/`.
      # The diffusion mod points the overworld at its own dim type instead
      # of `minecraft:overworld`, so the max-height override has to land at
      # the mod's namespaced path. Built off the vanilla overworld snapshot
      # so the lighting, fog, infiniburn, and audio attributes stay sane.
      files."data/terrain-diffusion-mc/dimension_type/terrain_diffusion.json" =
        ix.minecraft.dimensionType.withBase "terrain-diffusion-mc:terrain_diffusion"
        {
          base = "minecraft:overworld";
          min_y = worldHeight.minY;
          inherit (worldHeight) height;
          logical_height = worldHeight.height;
        };
    };

    # Path is relative to the server root. `level-name` defaults to `world`,
    # so the file lands at `world/data/terrain_diffusion_world_settings.dat`,
    # which is exactly where the mod's PersistentStateManager looks. On the
    # next deploy the symlink is re-asserted, which is the declarative
    # behaviour we want: scale is repo-owned, not drift-prone runtime state.
    serverFiles."world/data/terrain_diffusion_world_settings.dat" = {
      data = {
        scale = ix.minecraft.nbt.int worldScale;
        explicit_scale = ix.minecraft.nbt.bool true;
      };
    };
  };
}
