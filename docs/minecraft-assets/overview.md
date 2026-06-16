# minecraft-assets

`packages/minecraft-assets` is a Nix-only package (no Rust, no source code of its
own) that produces authentic Minecraft GUI textures and the vanilla bitmap font
by extracting them straight from Mojang's official `client.jar`. It exists so the
[desktop overlays](../bossbar-overlay/overview.md) render real Mojang art without
vendoring any copyrighted asset into this repo or trusting a third-party mirror.

- **Flake output:** `nix build .#minecraft-assets`. Also exposed in the package
  overlay (`pkgs.minecraft-assets`) so overlay derivations can `callPackage` it
  (`package.nix:6`).
- **Build kind:** a fixed-output derivation (`stdenvNoCC.mkDerivation`,
  `default.nix:37`), `platforms = all` (`default.nix:103`).

## What it extracts

The build unzips a pinned `client.jar` with `unzip -j` (flattening archive paths)
into a flat output layout (`default.nix:49`):

- `boss_bar/` - every PNG under `assets/minecraft/textures/gui/sprites/boss_bar/`
  (the color/progress/notch sprites the boss bar overlay layers).
- `gui/` - `book.png` plus the page-forward/backward and button widget sprites
  (`default.nix:65`), for the book overlay.
- `entity/experience_orb.png` - the 4x4 grid of 16x16 XP-orb icons, for the orb
  overlay (`default.nix:58`).
- `particle/angry.png` - the angry-villager puff (8x8), for the orb feed's
  failure pop (`default.nix:63`).
- `font/` - `ascii.png` (the basic-Latin face the overlays render with) plus
  `accented.png` and `nonlatin_european.png` for future coverage (`default.nix:79`).

Each `unzip` group fails the build if a path stops existing in a future jar, so a
silently missing sprite cannot slip through as an empty asset directory
(`default.nix:46`).

## Provenance and pinning

The DAG is `version_manifest_v2 -> <version>.json -> client.jar -> extraction`
(`default.nix:8`). Only the leaves that matter are pinned:

- `version = "1.21"` (`default.nix:28`).
- `client.jar` is fetched with `fetchurl` from a Mojang object URL whose path is
  Mojang's own SHA-1, with the matching Nix `sha256` hash (`default.nix:30`), so
  the fetch is reproducible and never trusts a mirror.

The refresh recipe (resolve a new `client.jar` from the manifest and read back
the hash) is documented inline at `default.nix:18`. Mojang's art is fetched at
build time and is not redistributed by this repository; the packaging itself is
MIT (`default.nix:102`).

## How consumers use it

The overlay build (`packages/bossbar-overlay/app/default.nix:73`) copies the
slices into each crate's `assets/` directory before compiling, where
`include_bytes!` bakes them into the binaries: `font/ascii.png` into
`overlay-core`, the boss bar sprites into `bossbar`, the gui sprites into `book`,
and the entity/particle sprites into `orb`. For local Rust development,
`packages/bossbar-overlay/app/scripts/fetch-assets.sh` nix-builds this package
and copies the same slices into the gitignored `assets/` trees. See the
[overlay engine](../bossbar-overlay/engine.md) for how the font sheet is measured
and rendered.

Note: this package extracts the GUI/font art for overlays; the separate Mojang
sound pack the overlays play through `minecraft-sound` is a different
fixed-output derivation (`packages/minecraft/sound/sounds.nix`, see
[minecraft tools](../minecraft/overview.md)).
