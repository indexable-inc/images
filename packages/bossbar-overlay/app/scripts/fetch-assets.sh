#!/usr/bin/env bash
# Download the Mojang art the overlay embeds at compile time: the vanilla boss
# bar sprite textures and the Minecraft "Mojangles" TTF.
#
# These are Mojang's (and Mojang-derived) art and are NOT redistributed in this
# repository (see .gitignore). This script pulls them, pinned to a version, into
# app/assets/ so `include_bytes!` finds them. Existing files are left alone, so
# re-running is cheap and works offline once the assets are present.
set -euo pipefail

MC_VERSION="1.21"
SPRITE_BASE="https://raw.githubusercontent.com/InventivetalentDev/minecraft-assets/${MC_VERSION}/assets/minecraft/textures/gui/sprites/boss_bar"
FONT_URL="https://github.com/tryashtar/minecraft-ttf/releases/download/v1.4/MinecraftDefault-Regular.ttf"

here="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
sprites="$here/assets/boss_bar"
fonts="$here/assets/fonts"
mkdir -p "$sprites" "$fonts"

colors="pink blue red green yellow purple white"
notches="notched_6 notched_10 notched_12 notched_20"

fetched=0
fetch_png() {
  local name="$1" out="$sprites/$1"
  [ -s "$out" ] && return 0
  curl -fsSL -o "$out" "$SPRITE_BASE/$name"
  if ! file "$out" | grep -q 'PNG image'; then
    rm -f "$out"
    echo "fetch-assets: $name did not download as a PNG" >&2
    exit 1
  fi
  fetched=$((fetched + 1))
}

for c in $colors; do
  fetch_png "${c}_background.png"
  fetch_png "${c}_progress.png"
done
for n in $notches; do
  fetch_png "${n}_background.png"
  fetch_png "${n}_progress.png"
done

font="$fonts/MinecraftDefault-Regular.ttf"
if [ ! -s "$font" ]; then
  curl -fsSL -o "$font" "$FONT_URL"
  fetched=$((fetched + 1))
fi

echo "fetch-assets: assets ready in app/assets (Minecraft ${MC_VERSION}, ${fetched} new)"
