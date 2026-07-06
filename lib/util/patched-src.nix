# Apply an in-repo, ordered patch series to an upstream source tree. This is
# the de-forking counterpart of `lib/util/pins.nix`: instead of maintaining a
# separate `indexable-inc` fork repo, a package pins the upstream at a
# `flake = false` input and keeps its delta as `*.patch` files in a sibling
# `patches/` folder, applied `0001..NNNN` on top of the pinned source.
#
# The patch folder is the single source of truth: the series is derived from
# `builtins.readDir patchDir` (naturally sorted, `*.patch` only), never a
# hand-kept list, so adding a patch is dropping a numbered file in the folder
# and nothing else. `rebase-patches` (packages/rebase-patches) regenerates these
# files through a real `git rebase` when the pinned base moves, so plain `patch`
# application is always correct at build time (the series is exact against the
# pinned rev; the build never needs fuzzy or structural merging).
{
  lib,
  applyPatches,
}:
# Return the patched source derivation. `applyPatches` copies `src` and runs
# `patch` for each entry, so the result is a tiny, seconds-fast derivation that
# doubles as the `checks.<system>.patched-src-<name>` conflict gate: if it
# builds, the series still applies.
{
  name,
  src,
  patchDir,
}:
applyPatches {
  name = "${name}-patched";
  inherit src;
  patches = lib.pipe (builtins.readDir patchDir) [
    (lib.filterAttrs (f: t: t == "regular" && lib.hasSuffix ".patch" f))
    builtins.attrNames
    lib.naturalSort
    (map (f: patchDir + "/${f}"))
  ];
}
