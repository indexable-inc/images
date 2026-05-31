#!/usr/bin/env bash
# Download the vanilla Minecraft boss bar sprite textures the overlay renders.
#
# These textures are Mojang's art and are NOT redistributed in this repository
# (see .gitignore). This script pulls them, pinned to a Minecraft version, from
# the InventivetalentDev/minecraft-assets mirror into src/assets/boss_bar/.
# Existing files are left alone, so re-running is cheap and works offline once
# the assets are present.
set -euo pipefail

VERSION="1.21"
BASE="https://raw.githubusercontent.com/InventivetalentDev/minecraft-assets/${VERSION}/assets/minecraft/textures/gui/sprites/boss_bar"

here="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
dest="$here/src/assets/boss_bar"
mkdir -p "$dest"

colors="pink blue red green yellow purple white"
notches="notched_6 notched_10 notched_12 notched_20"

fetched=0
fetch() {
  local name="$1"
  local out="$dest/$name"
  [ -s "$out" ] && return 0
  curl -fsSL -o "$out" "$BASE/$name"
  if ! file "$out" | grep -q 'PNG image'; then
    rm -f "$out"
    echo "fetch-assets: $name did not download as a PNG" >&2
    exit 1
  fi
  fetched=$((fetched + 1))
}

for c in $colors; do
  fetch "${c}_background.png"
  fetch "${c}_progress.png"
done
for n in $notches; do
  fetch "${n}_background.png"
  fetch "${n}_progress.png"
done

echo "fetch-assets: boss bar sprites ready in src/assets/boss_bar (Minecraft ${VERSION}, ${fetched} new)"
