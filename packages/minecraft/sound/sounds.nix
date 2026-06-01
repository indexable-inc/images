# Minecraft sound asset pack.
#
# !!! LICENSING / DO NOT UPLOAD !!!
# The `.ogg` files this derivation produces are Mojang's proprietary Minecraft
# assets (governed by the Minecraft EULA). This is a fixed-output derivation
# that DOWNLOADS them from Mojang's official CDN at build time on the building
# machine; it does not vendor them in this repo. The resulting store path
# therefore contains Mojang content and MUST NOT be pushed to any shared or
# public binary cache (e.g. `indexable-inc.cachix.org`), nor included in any
# image closure that gets cached or distributed. Each machine that wants
# sounds builds this path locally from Mojang. Keep it out of CI cache pushes.
{
  lib,
  stdenvNoCC,
  fetchurl,
  cacert,
  curl,
  jaq,
}:
let
  lock = lib.importJSON ./sounds/lock.json;

  # The asset index lists every sound object with its content hash. Pinned by
  # sha1 (the hash Mojang publishes for it), so this fetch is reproducible.
  assetIndex = fetchurl {
    inherit (lock.assetIndex) url hash;
  };

  # Sound names whose leading path segment matches are dropped. Defaults to the
  # multi-megabyte background music and music-disc tracks, which a sound-effect
  # player never needs. Widen `excludePrefixes` in lock.json to trim more.
  excludeRegex = "^(" + lib.concatStringsSep "|" lock.excludePrefixes + ")/";
in
stdenvNoCC.mkDerivation {
  pname = "minecraft-sound-assets";
  version = lock.minecraftVersion;

  dontUnpack = true;
  strictDeps = true;
  nativeBuildInputs = [
    curl
    jaq
  ];

  SSL_CERT_FILE = "${cacert}/etc/ssl/certs/ca-bundle.crt";

  # Fixed-output derivation: network access is permitted because the whole
  # assembled tree is content-addressed by `outputHash`, which is the integrity
  # guarantee for every downloaded file together. Refresh it with
  # `nix run .#update-sounds` after bumping the pinned Minecraft version.
  outputHashMode = "recursive";
  outputHashAlgo = "sha256";
  outputHash = lock.packHash;

  buildPhase = ''
    runHook preBuild

    list="$TMPDIR/sounds.tsv"
    jaq -r --arg ex ${lib.escapeShellArg excludeRegex} '
      .objects
      | to_entries[]
      | select((.key | startswith("minecraft/sounds/")) and (.key | endswith(".ogg")))
      | (.key | ltrimstr("minecraft/sounds/")) as $name
      | select(($name | test($ex)) | not)
      | "\(.value.hash) \($name)"
    ' ${assetIndex} > "$list"

    echo "downloading $(wc -l < "$list") Minecraft sounds from Mojang's CDN..."

    mkdir -p "$out/sounds"
    cut -d' ' -f2 "$list" | xargs -n1 dirname | sort -u | while read -r dir; do
      mkdir -p "$out/sounds/$dir"
    done

    export out
    xargs -P 16 -n 2 bash -c '
      hash="$1"; name="$2"
      curl -sSfL --retry 3 \
        "https://resources.download.minecraft.net/''${hash:0:2}/$hash" \
        -o "$out/sounds/$name"
    ' _ < "$list"

    runHook postBuild
  '';

  dontInstall = true;

  meta = {
    description = "Minecraft sound effects (downloaded from Mojang's CDN at build time)";
    # License intentionally omitted: these are Mojang's proprietary assets.
    # See the DO NOT UPLOAD banner at the top of this file.
  };
}
