#!/usr/bin/env nu

# Refresh the pinned Minecraft sound pack for `packages/minecraft/minecraft/sound`.
#
# Resolves a Minecraft version from Mojang's launcher manifest, pins its asset
# index (the file that lists every sound object and its content hash), then
# rebuilds the fixed-output `sounds.nix` derivation to recompute the aggregate
# `packHash`. All three pins land in `packages/minecraft/minecraft/sound/sounds/lock.json`.
#
# The sounds themselves are Mojang's proprietary assets and are never vendored
# in the repo; only the manifest pins live here. See sounds.nix for the
# do-not-upload constraint on the built store path.

def main [
  --version (-v): string  # MC version id (defaults to the latest release)
] {
  let root = (^git rev-parse --show-toplevel | str trim)
  let lock_path = ($root | path join "packages/minecraft/minecraft/sound/sounds/lock.json")
  let sounds_nix = ($root | path join "packages/minecraft/minecraft/sound/sounds.nix")

  let manifest_url = "https://launchermeta.mojang.com/mc/game/version_manifest_v2.json"
  print "fetching launcher manifest..."
  let manifest = (http get $manifest_url)

  let mc_version = ($version | default $manifest.latest.release)
  let entry = ($manifest.versions | where id == $mc_version)
  if ($entry | is-empty) {
    error make { msg: $"version '($mc_version)' not found in manifest" }
  }

  print $"fetching ($mc_version) metadata..."
  let meta = (http get ($entry | first | get url))
  let asset_index = $meta.assetIndex
  let index_sri = (^nix hash convert --hash-algo sha1 --to sri $asset_index.sha1 | str trim)

  # Preserve the existing exclude list (defaults to the bulky music/records
  # tracks a sound-effect player never needs) across version bumps.
  let excludes = if ($lock_path | path exists) {
    (open $lock_path | get excludePrefixes? | default ["music" "records"])
  } else {
    ["music" "records"]
  }

  # The fake hash forces a rebuild so Nix reports the real aggregate hash.
  let fake = "sha256-AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA="
  let base = {
    minecraftVersion: $mc_version
    assetIndexId: $asset_index.id
    assetIndex: { url: $asset_index.url, hash: $index_sri }
    excludePrefixes: $excludes
  }
  $base | merge { packHash: $fake } | to json --indent 2 | save --force $lock_path

  let system = (^nix eval --impure --raw --expr "builtins.currentSystem")
  let expr = $"let f = builtins.getFlake \"path:($root)\"; in f.inputs.nixpkgs.legacyPackages.\"($system)\".callPackage ($sounds_nix) { }"
  print $"building sound pack for ($mc_version) to compute packHash (downloads from Mojang)..."
  let res = (do { ^nix build --impure --no-link --expr $expr } | complete)

  let got = (
    $res.stderr
    | parse --regex 'got:\s+(?<h>sha256-[A-Za-z0-9+/=]+)'
    | get h?
    | default []
  )
  let pack_hash = if ($got | is-empty) {
    if $res.exit_code == 0 {
      # Already reproduced the pinned bytes; keep the current hash.
      (open $lock_path | get packHash)
    } else {
      error make { msg: $"nix build failed without a hash mismatch:\n($res.stderr)" }
    }
  } else {
    ($got | first)
  }

  $base | merge { packHash: $pack_hash } | to json --indent 2 | save --force $lock_path
  print $"updated ($lock_path):"
  print $"  minecraft ($mc_version), asset index ($asset_index.id), packHash ($pack_hash)"
}
