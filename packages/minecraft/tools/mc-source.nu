#!/usr/bin/env nu

# Decompile a Minecraft server (or client) jar with Mojang's deobfuscation
# mappings, using Vineflower. The default version is 1.21.11 — the most
# recent release whose server jar still ships `server_mappings` from Mojang.
# Snapshots and 26.x+ releases stopped publishing mappings, so versions
# after that decompile to obfuscated class names like `a.b.Foo` unless a
# community mapping (Yarn, Mojmap-back-port) is bridged in by hand.

def main [
  --version (-v): string = "1.21.11"  # MC version id from the launcher manifest
  --output  (-o): path   = "minecraft-source"
  --side    (-s): string = "server"   # "server" or "client"
  --jar-only                          # skip decompile; emit the deobfuscated jar + mappings only
] {
  let manifest_url = "https://launchermeta.mojang.com/mc/game/version_manifest_v2.json"
  print $"fetching launcher manifest..."
  let manifest = (http get $manifest_url)

  let entry = ($manifest.versions | where id == $version)
  if ($entry | is-empty) {
    error make { msg: $"version '($version)' not found in manifest" }
  }

  let meta_url = ($entry | first | get url)
  print $"fetching ($version) metadata..."
  let meta = (http get $meta_url)
  let downloads = $meta.downloads

  if not ($side in ($downloads | columns)) {
    let available = ($downloads | columns | where {|c| not ($c | str ends-with "_mappings")} | str join ", ")
    error make { msg: $"side '($side)' not in downloads; available: ($available)" }
  }

  let map_key = $"($side)_mappings"
  let has_mappings = ($map_key in ($downloads | columns))

  let workdir = (mktemp -d)
  print $"working in ($workdir)"

  let jar = ($workdir | path join $"($side).jar")
  let jar_url = ($downloads | get $side | get url)
  print $"downloading ($side) jar (($downloads | get $side | get size | into filesize))..."
  http get $jar_url | save --force $jar

  mut mapping_args = []
  if $has_mappings {
    let mappings = ($workdir | path join $"($side)-mappings.txt")
    let map_url = ($downloads | get $map_key | get url)
    print $"downloading Mojang mappings (($downloads | get $map_key | get size | into filesize))..."
    http get $map_url | save --force $mappings
    $mapping_args = [$"-mpp=($mappings)"]
  } else {
    print $"WARNING: ($version) does not ship ($side)_mappings."
    print "         Output classes will be obfuscated (a.b.Foo, etc.)."
    print "         Mojang stopped publishing mappings around the 26.x line;"
    print "         use --version 1.21.11 to get the most recent mapped release."
  }

  mkdir $output
  if $jar_only {
    cp $jar ($output | path join $"($side).jar")
    if $has_mappings {
      cp ($workdir | path join $"($side)-mappings.txt") ($output | path join $"($side)-mappings.txt")
    }
    print $"saved jar + mappings to ($output)"
    return
  }

  print "running vineflower (this can take several minutes for the full server)..."
  vineflower ...$mapping_args $jar $output
  print $"decompiled ($version) ($side) source in ($output)"
}
