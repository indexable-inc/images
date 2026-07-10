# Vanilla dimension-type JSON snapshots, keyed by Minecraft resource id.
#
# Schema target: Minecraft Java 1.21.10+ (the `attributes` map shape), matching
# the default `services.minecraft.version` in `packages/minecraft/catalogs/versions.nix`.
# The height knobs (`min_y`, `height`, `logical_height`) are deliberately omitted
# so callers can supply them and have `ix.minecraft.dimensionType.withBase` merge
# the rest in.
#
# Refresh procedure: unpack the relevant server jar and copy
# `data/minecraft/dimension_type/{overworld,the_nether,the_end}.json` into the
# attrsets below, then re-strip `min_y` / `height` / `logical_height`. No public
# Mojang URL serves the raw JSON outside the jar, so the snapshot is hand-tracked.
{
  "minecraft:overworld" = {
    ambient_light = 0.0;
    attributes = {
      "minecraft:audio/ambient_sounds".mood = {
        block_search_extent = 8;
        offset = 2.0;
        sound = "minecraft:ambient.cave";
        tick_delay = 6000;
      };
      "minecraft:audio/background_music" = {
        creative = {
          max_delay = 24000;
          min_delay = 12000;
          sound = "minecraft:music.creative";
        };
        default = {
          max_delay = 24000;
          min_delay = 12000;
          sound = "minecraft:music.game";
        };
      };
      "minecraft:gameplay/bed_rule" = {
        can_set_spawn = "always";
        can_sleep = "when_dark";
        error_message.translate = "block.minecraft.bed.no_sleep";
      };
      "minecraft:gameplay/nether_portal_spawns_piglin" = true;
      "minecraft:gameplay/respawn_anchor_works" = false;
      "minecraft:visual/ambient_light_color" = "#0a0a0a";
      "minecraft:visual/cloud_color" = "#ccffffff";
      "minecraft:visual/cloud_height" = 192.33;
      "minecraft:visual/fog_color" = "#c0d8ff";
      "minecraft:visual/sky_color" = "#78a7ff";
    };
    coordinate_scale = 1.0;
    default_clock = "minecraft:overworld";
    has_ceiling = false;
    has_ender_dragon_fight = false;
    has_skylight = true;
    infiniburn = "#minecraft:infiniburn_overworld";
    monster_spawn_block_light_limit = 0;
    monster_spawn_light_level = {
      type = "minecraft:uniform";
      max_inclusive = 7;
      min_inclusive = 0;
    };
    timelines = "#minecraft:in_overworld";
  };

  "minecraft:the_nether" = {
    ambient_light = 0.1;
    attributes = {
      "minecraft:gameplay/bed_rule" = {
        can_set_spawn = "never";
        can_sleep = "never";
        explodes = true;
      };
      "minecraft:gameplay/can_start_raid" = false;
      "minecraft:gameplay/fast_lava" = true;
      "minecraft:gameplay/piglins_zombify" = false;
      "minecraft:gameplay/respawn_anchor_works" = true;
      "minecraft:gameplay/sky_light_level" = 4.0;
      "minecraft:gameplay/snow_golem_melts" = true;
      "minecraft:gameplay/water_evaporates" = true;
      "minecraft:visual/ambient_light_color" = "#302821";
      "minecraft:visual/default_dripstone_particle".type = "minecraft:dripping_dripstone_lava";
      "minecraft:visual/fog_end_distance" = 96.0;
      "minecraft:visual/fog_start_distance" = 10.0;
      "minecraft:visual/sky_light_color" = "#7a7aff";
      "minecraft:visual/sky_light_factor" = 0.0;
    };
    cardinal_light = "nether";
    coordinate_scale = 8.0;
    has_ceiling = true;
    has_ender_dragon_fight = false;
    has_fixed_time = true;
    has_skylight = false;
    infiniburn = "#minecraft:infiniburn_nether";
    monster_spawn_block_light_limit = 15;
    monster_spawn_light_level = 7;
    skybox = "none";
    timelines = "#minecraft:in_nether";
  };

  "minecraft:the_end" = {
    ambient_light = 0.25;
    attributes = {
      "minecraft:audio/ambient_sounds".mood = {
        block_search_extent = 8;
        offset = 2.0;
        sound = "minecraft:ambient.cave";
        tick_delay = 6000;
      };
      "minecraft:audio/background_music".default = {
        max_delay = 24000;
        min_delay = 6000;
        replace_current_music = true;
        sound = "minecraft:music.end";
      };
      "minecraft:gameplay/bed_rule" = {
        can_set_spawn = "never";
        can_sleep = "never";
        explodes = true;
      };
      "minecraft:gameplay/respawn_anchor_works" = false;
      "minecraft:visual/ambient_light_color" = "#3f473f";
      "minecraft:visual/fog_color" = "#181318";
      "minecraft:visual/sky_color" = "#000000";
      "minecraft:visual/sky_light_color" = "#ac60cd";
      "minecraft:visual/sky_light_factor" = 0.0;
    };
    coordinate_scale = 1.0;
    default_clock = "minecraft:the_end";
    has_ceiling = false;
    has_ender_dragon_fight = true;
    has_fixed_time = true;
    has_skylight = true;
    infiniburn = "#minecraft:infiniburn_end";
    monster_spawn_block_light_limit = 0;
    monster_spawn_light_level = 15;
    skybox = "end";
    timelines = "#minecraft:in_end";
  };
}
