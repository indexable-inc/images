# Pushes overworld/nether/end to Java's maximum 4064-block world (min_y -2032,
# height 4064). Only the height knobs are listed; vanilla dimension-type fields
# come from `ix.minecraft.dimensionType.defaults` via the `base` field.
let
  minY = -2032;
  height = 4064;
  maxHeightOverride = base: {
    inherit base;
    min_y = minY;
    inherit height;
    # logical_height defaults to height when unset, but spell it out so the
    # generated JSON is greppable.
    logical_height = height;
  };
in {
  inherit minY height;
  topY = minY + height - 1;

  pack = {
    description = "ix Factions max-height dimensions";
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
