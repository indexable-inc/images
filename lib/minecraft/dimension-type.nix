{ lib }:
let
  inherit (import ../util/deep-merge.nix { inherit lib; }) rhs;

  defaults = import ./dimension-type-defaults.nix;
  bases = lib.attrNames defaults;

  # MC dimension-type alignment & range. The 16-block alignment comes from
  # chunk sections; the [-2032, 2031] band and 4064 height cap are Java's
  # hard limits. logical_height must be a multiple of 16 and not exceed height.
  alignmentStep = 16;
  minYHardFloor = -2032;
  minYHardCeil = 2031;
  maxHeight = 4064;

  divisibleBy = step: value: lib.mod value step == 0;

  validateHeights =
    name: rendered:
    let
      minY = rendered.min_y or null;
      height = rendered.height or null;
      logicalHeight = rendered.logical_height or null;
      checks = [
        {
          assertion = minY != null;
          message = "services.minecraft.datapacks.<n>.dimensionTypes.${name}: min_y is required (no default available; set explicitly or via `base`).";
        }
        {
          assertion = height != null;
          message = "services.minecraft.datapacks.<n>.dimensionTypes.${name}: height is required.";
        }
        {
          assertion = minY == null || divisibleBy alignmentStep minY;
          message = "services.minecraft.datapacks.<n>.dimensionTypes.${name}: min_y (${toString minY}) must be a multiple of ${toString alignmentStep}.";
        }
        {
          assertion = height == null || divisibleBy alignmentStep height;
          message = "services.minecraft.datapacks.<n>.dimensionTypes.${name}: height (${toString height}) must be a multiple of ${toString alignmentStep}.";
        }
        {
          assertion = minY == null || (minY >= minYHardFloor && minY <= minYHardCeil);
          message = "services.minecraft.datapacks.<n>.dimensionTypes.${name}: min_y (${toString minY}) must be within [${toString minYHardFloor}, ${toString minYHardCeil}].";
        }
        {
          assertion = height == null || (height >= alignmentStep && height <= maxHeight);
          message = "services.minecraft.datapacks.<n>.dimensionTypes.${name}: height (${toString height}) must be within [${toString alignmentStep}, ${toString maxHeight}].";
        }
        {
          assertion = (minY == null || height == null) || (minY + height <= minYHardCeil + 1);
          message =
            let
              sum = if minY != null && height != null then minY + height else 0;
            in
            "services.minecraft.datapacks.<n>.dimensionTypes.${name}: min_y + height (${toString sum}) must not exceed ${toString (minYHardCeil + 1)}.";
        }
        {
          assertion = logicalHeight == null || divisibleBy alignmentStep logicalHeight;
          message = "services.minecraft.datapacks.<n>.dimensionTypes.${name}: logical_height (${toString logicalHeight}) must be a multiple of ${toString alignmentStep}.";
        }
        {
          assertion = logicalHeight == null || height == null || logicalHeight <= height;
          message = "services.minecraft.datapacks.<n>.dimensionTypes.${name}: logical_height (${toString logicalHeight}) must not exceed height (${toString height}).";
        }
      ];
    in
    # Assert every check (each carries its own message), then return `rendered`.
    # Matches the `assert lib.assertMsg ...` idiom the rest of lib/ uses; the
    # old `lib.checkAssertWarn` helper was removed in the lib/builtins refactor.
    lib.foldl' (val: c: assert lib.assertMsg c.assertion c.message; val) rendered checks;

  # Project a dimensionTypes submodule value to the JSON written to disk: strip
  # the `base` field, merge the named vanilla snapshot underneath, default
  # logical_height to height when unset, and validate heights.
  withBase =
    name: value:
    let
      base = value.base or null;
      overrides = removeAttrs value [ "base" ];
      baseDefaults = if base == null then { } else defaults.${base};
      merged = rhs baseDefaults overrides;
      withLogical = merged // {
        logical_height = merged.logical_height or (merged.height or null);
      };
    in
    validateHeights name withLogical;
in
{
  inherit
    defaults
    bases
    withBase
    ;
}
