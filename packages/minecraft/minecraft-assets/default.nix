{
  ix,
  lib,
  stdenvNoCC,
  fetchurl,
  unzip,
}:
# Authentic Minecraft art, extracted straight from Mojang's official client jar
# rather than vendored or pulled from a third-party mirror. The DAG is:
#
#   version_manifest_v2  ->  <version>.json  ->  client.jar  ->  this extraction
#
# We pin the two leaves that matter. `client.jar` is content-addressed by
# Mojang's own SHA-1 (the object URL *is* the sha1), so `fetchurl` with the
# matching Nix hash is fully reproducible and never trusts a mirror. The
# extraction below is pure: it only unzips the fixed jar and selects the exact
# texture and bitmap-font paths the overlays embed.
#
# Refresh recipe (when bumping `version`): resolve the new client.jar url from
# the manifest, put it in pins.json, then rebuild and copy the `got:` hash from
# the mismatch error (this package carries no registry updateScript, so
# `nix run .#update` does not touch it):
#   v=$(curl -fsSL https://launchermeta.mojang.com/mc/game/version_manifest_v2.json \
#        | jq -r '.versions[]|select(.id=="VERSION").url')
#   curl -fsSL "$v" | jq -r .downloads.client.url   # -> pins.json `url`
#
# Mojang's art is NOT redistributed by this repository; it is fetched at build
# time, the same boundary the rest of the repo uses for upstream binaries.
let
  # Version + client.jar URL and SRI hash live in the sibling pins.json, never
  # inline (repo policy).
  pin = ix.pins.loadPin ./pins.json "client-jar";
  inherit (pin) version;

  clientJar = fetchurl {inherit (pin) url hash;};

  tex = "assets/minecraft/textures";
in
  stdenvNoCC.mkDerivation {
    pname = "minecraft-assets";
    inherit version;

    dontUnpack = true;
    strictDeps = true;
    nativeBuildInputs = [unzip];

    # `unzip -j` flattens the archive paths into our own flat layout; the `*.png`
    # globs also drop the sibling `.mcmeta` files we do not use. Each group fails
    # loudly (`unzip` exits non-zero) if a path stops existing in a future jar, so
    # a silently missing sprite cannot slip through as an empty asset dir.
    buildPhase = ''
      # shell
      runHook preBuild

      mkdir -p "$out/boss_bar" "$out/gui" "$out/font" "$out/entity" "$out/particle"

      unzip -j -o ${clientJar} '${tex}/gui/sprites/boss_bar/*.png' -d "$out/boss_bar"

      # The experience-orb sprite sheet (16x16 icons in a 4x4 grid) for the XP orb
      # overlay. It lives under `entity/`, not the GUI sprites.
      unzip -j -o ${clientJar} '${tex}/entity/experience_orb.png' -d "$out/entity"

      # The angry-villager particle (the grey "displeased / can't trade" puff, 8x8)
      # for the failure pop in the karma feed overlay. The `angry_villager` particle
      # references texture `minecraft:angry`, i.e. `particle/angry.png`.
      unzip -j -o ${clientJar} '${tex}/particle/angry.png' -d "$out/particle"

      unzip -j -o ${clientJar} \
        '${tex}/gui/book.png' \
        '${tex}/gui/sprites/widget/page_forward.png' \
        '${tex}/gui/sprites/widget/page_forward_highlighted.png' \
        '${tex}/gui/sprites/widget/page_backward.png' \
        '${tex}/gui/sprites/widget/page_backward_highlighted.png' \
        '${tex}/gui/sprites/widget/button.png' \
        '${tex}/gui/sprites/widget/button_highlighted.png' \
        '${tex}/gui/sprites/widget/button_disabled.png' \
        -d "$out/gui"

      # The vanilla bitmap font sheets. `ascii.png` is the basic-Latin face the
      # overlays render with; the others are kept so a consumer can extend coverage
      # without re-deriving this package.
      unzip -j -o ${clientJar} \
        '${tex}/font/ascii.png' \
        '${tex}/font/accented.png' \
        '${tex}/font/nonlatin_european.png' \
        -d "$out/font"

      runHook postBuild
    '';

    dontInstall = true;
    dontFixup = true;

    meta = {
      description = "Authentic Minecraft GUI textures and bitmap font, extracted from Mojang's official client jar";
      longDescription = ''
        A reproducible extraction of the boss bar sprites, the book GUI texture and
        page widgets, the experience-orb sheet, the angry-villager particle, and the
        vanilla bitmap font from the official Minecraft
        ${version} client jar. Consumed by the desktop overlays so they render real
        Mojang art instead of a hand-vendored or mirrored copy.
      '';
      # Mojang's art is fetched at build time and not redistributed by this repo;
      # the packaging here is MIT (Copyright 2026 Indexable Inc.).
      license = lib.licenses.mit;
      platforms = lib.platforms.all;
    };
  }
