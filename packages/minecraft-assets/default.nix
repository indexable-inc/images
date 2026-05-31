{
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
# Refresh recipe (when bumping `version`): resolve the new client.jar from the
# manifest and read back the hash Nix wants:
#   v=$(curl -fsSL https://launchermeta.mojang.com/mc/game/version_manifest_v2.json \
#        | jq -r '.versions[]|select(.id=="VERSION").url')
#   url=$(curl -fsSL "$v" | jq -r .downloads.client.url)
#   nix store prefetch-file --json "$url" | jq -r .hash
#
# Mojang's art is NOT redistributed by this repository; it is fetched at build
# time, the same boundary the rest of the repo uses for upstream binaries.
let
  version = "1.21";

  clientJar = fetchurl {
    url = "https://piston-data.mojang.com/v1/objects/0e9a07b9bb3390602f977073aa12884a4ce12431/client.jar";
    hash = "sha256-S1a8m0Tefoj5+7UucWwYJA2jRGn8uvpe0udbBNYGF+g=";
  };

  tex = "assets/minecraft/textures";
in
stdenvNoCC.mkDerivation {
  pname = "minecraft-assets";
  inherit version;

  dontUnpack = true;
  strictDeps = true;
  nativeBuildInputs = [ unzip ];

  # `unzip -j` flattens the archive paths into our own flat layout; the `*.png`
  # globs also drop the sibling `.mcmeta` files we do not use. Each group fails
  # loudly (`unzip` exits non-zero) if a path stops existing in a future jar, so
  # a silently missing sprite cannot slip through as an empty asset dir.
  buildPhase = ''
    runHook preBuild

    mkdir -p "$out/boss_bar" "$out/gui" "$out/font"

    unzip -j -o ${clientJar} '${tex}/gui/sprites/boss_bar/*.png' -d "$out/boss_bar"

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
      page widgets, and the vanilla bitmap font from the official Minecraft
      ${version} client jar. Consumed by the desktop overlays so they render real
      Mojang art instead of a hand-vendored or mirrored copy.
    '';
    # Mojang's art is fetched at build time and not redistributed by this repo;
    # the packaging here is MIT (Copyright 2026 Indexable Inc.).
    license = lib.licenses.mit;
    platforms = lib.platforms.all;
  };
}
