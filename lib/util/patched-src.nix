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
# `applyPatches` is a nixpkgs trivial builder and opts out of substitution. A
# patched source (e.g. ghostty) enters the darwin cross lane's eval-time IFD
# closure, so a Mac must be able to substitute it: `evalTimeSubstitutable`
# (threaded from lib) flips `allowSubstitutes` back on. See its doc comment.
{
  lib,
  applyPatches,
  evalTimeSubstitutable,
}:
# Return the patched source derivation. `applyPatches` copies `src` and runs
# `patch` for each entry, so the result is a tiny, seconds-fast derivation that
# doubles as the `checks.<system>.patched-src-<name>` conflict gate: if it
# builds, the series still applies.
{
  name,
  src,
  patchDir,
}: let
  # `rebase-patches` exports the series with `git format-patch --zero-commit
  # --no-signature --no-stat -N`, so a conforming file is byte-stable across
  # regenerations: no commit-hash, `[PATCH N/M]` count, diffstat, or
  # git-version churn in the diff. Hand-added patches drift from that shape,
  # so assert it at eval and fail with the regeneration command.
  zeroFrom = "From 0000000000000000000000000000000000000000 Mon Sep 17 00:00:00 2001";
  assertCanonical = fileName: path: let
    lines = lib.splitString "\n" (builtins.readFile path);
    nonEmpty = lib.filter (l: l != "") lines;
    # Diffstat can only appear in the header, between the `---` separator
    # and the first `diff --git`; scoping the scan there keeps diff content
    # that merely mentions "files changed" from tripping the check.
    firstDiffLine = lib.lists.findFirstIndex (lib.hasPrefix "diff --git ") (lib.length lines) lines;
    header = lib.take firstDiffLine lines;
    fail = why:
      throw ''
        ${name}: ${fileName}: ${why}.
        The series must match its writer, `git format-patch --zero-commit
        --no-signature --no-stat -N`; regenerate it with `nix run .#rebase-patches`.
      '';
  in
    if builtins.match "[0-9]{4}-.*" fileName == null
    then fail "filename lacks the NNNN- series prefix"
    else if lib.head lines != zeroFrom
    then fail "first line is not the zeroed `From ` header (--zero-commit)"
    else if !(lib.any (lib.hasPrefix "Subject: [PATCH] ") lines)
    then fail "missing unnumbered `Subject: [PATCH] ` header (-N; `[PATCH N/M]` renumbers the whole series on insert)"
    else if lib.any (l: builtins.match " [0-9]+ files? changed.*" l != null) header
    then fail "diffstat block in the header (--no-stat)"
    else if lib.length nonEmpty >= 2 && lib.elemAt nonEmpty (lib.length nonEmpty - 2) == "-- "
    then fail "trailing signature block (--no-signature)"
    else path;
in
  evalTimeSubstitutable (applyPatches {
    name = "${name}-patched";
    inherit src;
    patches = lib.pipe (builtins.readDir patchDir) [
      (lib.filterAttrs (f: t: t == "regular" && lib.hasSuffix ".patch" f))
      builtins.attrNames
      lib.naturalSort
      (map (f: assertCanonical f (patchDir + "/${f}")))
    ];
  })
