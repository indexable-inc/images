# Pushes overworld/nether/end to Java's hard 4064-block limit (min_y -2032,
# height 4064, top y 2031). Only the height knobs are listed; the rest of
# each dimension type comes from `ix.minecraft.dimensionType.defaults`
# through the `base` field.
#
# The diffusion mod's own dimension type
# (`terrain-diffusion-mc:terrain_diffusion`) lives in a non-vanilla
# namespace, so its max-height override is written from `minecraft.nix`
# through `datapacks.<n>.files` instead of `dimensionTypes` (which only
# emits under `data/minecraft/dimension_type/`).
let
  minY = -2032;
  height = 4064;
  maxHeightOverride = base: {
    inherit base;
    min_y = minY;
    inherit height;
    logical_height = height;
  };
in {
  inherit minY height;
  topY = minY + height - 1;

  pack = {
    description = "ix Crazy Terrain max-height dimensions";
    min_format = [
      101
      1
    ];
    max_format = 101;
  };

  dimensionTypes = {
    overworld = maxHeightOverride "minecraft:overworld";
    the_nether = maxHeightOverride "minecraft:the_nether";
    the_end = maxHeightOverride "minecraft:the_end";
  };
}
