#!/usr/bin/env bash
# Populate the gitignored Minecraft art each crate embeds, for local `cargo`
# builds. The Nix build does this itself (see app/default.nix); this script is the
# `cargo`-only convenience.
#
# Source of truth is the `minecraft-assets` Nix derivation, which extracts the
# textures and bitmap font straight from Mojang's official client jar (pinned by
# Mojang's own hash). We just build it and copy the slices each crate needs.
#
# These are Mojang's art and are NOT redistributed in this repository (see
# .gitignore). Re-running is cheap; existing files are overwritten.
set -euo pipefail

here="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"   # .../app
repo_root="$(cd "$here/../../.." && pwd)"                  # repo flake root

assets="$(nix build "${repo_root}#minecraft-assets" --no-link --print-out-paths)"
echo "fetch-assets: minecraft-assets at $assets"

# overlay-core embeds the shared bitmap font; the apps embed their sprites.
mkdir -p "$here/crates/overlay-core/assets" \
         "$here/crates/bossbar/assets/boss_bar" \
         "$here/crates/book/assets/gui" \
         "$here/crates/orb/assets/entity" \
         "$here/crates/orb/assets/particle"

cp -f "$assets/font/ascii.png" "$here/crates/overlay-core/assets/ascii.png"
cp -f "$assets"/boss_bar/*.png "$here/crates/bossbar/assets/boss_bar/"
cp -f "$assets"/gui/*.png "$here/crates/book/assets/gui/"
cp -f "$assets"/entity/experience_orb.png "$here/crates/orb/assets/entity/"
cp -f "$assets"/particle/angry.png "$here/crates/orb/assets/particle/"

echo "fetch-assets: assets ready (font -> overlay-core, boss_bar -> bossbar, gui -> book, entity + particle -> orb)"
