#!/usr/bin/env bash
# Import a boss bar texture set from a user-supplied resource pack into the
# overlay's themes directory, so a bar row with `theme = '<name>'` renders from
# it (see crates/bossbar/src/theme.rs for the on-disk contract).
#
# This is the user-brings-their-own-pack counterpart to fetch-assets.sh: it
# never downloads anything and ships no art. You point it at a pack you are
# licensed to use (a .zip/.mcpack you bought or made, or an unpacked
# directory); it slices out the boss bar sprites and lays them down as
#
#   <themes root>/<name>/background.png
#   <themes root>/<name>/progress.png
#   <themes root>/<name>/notched_*_{background,progress}.png   (when present)
#
# Java resource packs keep per-color art under
# assets/minecraft/textures/gui/sprites/boss_bar/<color>_{background,progress}.png;
# `--color` picks which color slot's art becomes the theme's base pair
# (themed packs usually restyle every color, so import each color you want as
# its own theme name).
#
# Usage:
#   import-theme.sh <pack.zip|pack.mcpack|pack-dir> <theme-name> [--color pink]
#   BOSSBAR_THEMES=/elsewhere import-theme.sh ...     # override the destination
set -euo pipefail

die() { printf 'import-theme: %s\n' "$1" >&2; exit 1; }

[ "$#" -ge 2 ] || die "usage: import-theme.sh <pack.zip|dir> <theme-name> [--color pink]"
pack="$1"
name="$2"
shift 2
color="pink"
while [ "$#" -gt 0 ]; do
  case "$1" in
    --color)
      color="${2:-}"
      [ -n "$color" ] || die "--color needs a value"
      shift 2
      ;;
    *) die "unknown argument: $1" ;;
  esac
done

# Same single-path-component rule the overlay enforces on the `theme` column.
[[ "$name" =~ ^[A-Za-z0-9_-][A-Za-z0-9._-]*$ ]] \
  || die "theme name must be a plain directory name (letters, digits, . _ -)"

themes_root="${BOSSBAR_THEMES:-}"
if [ -z "$themes_root" ]; then
  case "$(uname -s)" in
    Darwin) themes_root="$HOME/Library/Application Support/bossbar-overlay/themes" ;;
    *) themes_root="${XDG_DATA_HOME:-$HOME/.local/share}/bossbar-overlay/themes" ;;
  esac
fi

workdir=""
cleanup() {
  if [ -n "$workdir" ]; then rm -rf "$workdir"; fi
}
trap cleanup EXIT

if [ -d "$pack" ]; then
  src_root="$pack"
elif [ -f "$pack" ]; then
  command -v unzip >/dev/null || die "unzip is required to read $pack"
  workdir="$(mktemp -d)"
  unzip -q "$pack" -d "$workdir" || die "could not unzip $pack"
  src_root="$workdir"
else
  die "no such pack: $pack"
fi

# The pack layout, or the same files at the pack root for a bare directory of
# sprites someone already sliced.
sprites="$src_root/assets/minecraft/textures/gui/sprites/boss_bar"
[ -d "$sprites" ] || sprites="$src_root"

bg="$sprites/${color}_background.png"
fill="$sprites/${color}_progress.png"
# A bare sprite directory may already use the theme-contract names.
[ -f "$bg" ] || bg="$sprites/background.png"
[ -f "$fill" ] || fill="$sprites/progress.png"
[ -f "$bg" ] && [ -f "$fill" ] \
  || die "no ${color}_background.png/${color}_progress.png (or background.png/progress.png) under $sprites"

dest="$themes_root/$name"
mkdir -p "$dest"
cp -f "$bg" "$dest/background.png"
cp -f "$fill" "$dest/progress.png"
copied=2
for stem in notched_6 notched_10 notched_12 notched_20; do
  if [ -f "$sprites/${stem}_background.png" ] && [ -f "$sprites/${stem}_progress.png" ]; then
    cp -f "$sprites/${stem}_background.png" "$dest/${stem}_background.png"
    cp -f "$sprites/${stem}_progress.png" "$dest/${stem}_progress.png"
    copied=$((copied + 2))
  fi
done

echo "import-theme: $copied sprite(s) -> $dest"
echo "import-theme: try it:  ./bossbar set <id|title> --theme $name"
