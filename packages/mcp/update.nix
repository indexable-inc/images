{
  nix,
  writeNushellApplication,
}:
writeNushellApplication {
  name = "mcp-pypi-pins-update";
  runtimeInputs = [nix];
  meta.description = "Refresh packages/mcp/pins.json from PyPI release metadata";
  text = ''
    # nu
    # Run from the repo root: `nix run .#mcp.updateScript`.
    # Policy markers in pins.json:
    # - `prefetch = "manual"`: keep URL/hash hand-owned for non-sdist artifacts.
    # - `hold = "<reason>"`: keep the current version, URL, and hash.
    # - `track = "<dotted prefix>"`: update only within that version line.
    def source-url [project: string, filename: string] {
      let first = ($project | str substring 0..0)
      $"https://pypi.io/packages/source/($first)/($project)/($filename)"
    }

    def sri-from-hex [hex: string] {
      ^nix hash convert --hash-algo sha256 --to sri $hex | str trim
    }

    # A pin candidate is a plain dotted-integer version. PyPI release lists
    # also carry pre-releases (3.5.6.dev1, 4.0.0rc1); those never become pins,
    # so they are disqualified here rather than crashing `into int`.
    def version-segments [version: string] {
      let segments = ($version | split row ".")
      if ($segments | all {|segment| $segment =~ '^[0-9]+$' }) {
        $segments | each { into int }
      } else {
        null
      }
    }

    def version-matches-track [version: string, track: string] {
      let version_segments = (version-segments $version)
      let track_segments = (version-segments $track)
      if ($version_segments == null) or ($track_segments == null) {
        false
      } else if (($version_segments | length) < ($track_segments | length)) {
        false
      } else {
        ($version_segments | first ($track_segments | length)) == $track_segments
      }
    }

    def tracked-version [name: string, releases: record, track: string] {
      let versions = (
        $releases
        | columns
        | where {|version| version-matches-track $version $track }
        | sort-by {|version| version-segments $version }
      )
      if ($versions | is-empty) {
        error make {msg: $"($name): no PyPI releases match track ($track)"}
      }
      $versions | last
    }

    def refresh-pin [name: string, entry: record] {
      # pins.json supports three updater policy markers:
      #
      # - `prefetch = "manual"`: hash-mode hold for platform-specific artifacts
      #   whose URL/hash must be refreshed by hand.
      # - `hold = "<reason>"`: version hold for pins whose dependency override set
      #   is hand-tuned to one exact upstream release.
      # - `track = "<version prefix>"`: version-line tracking for packages that
      #   should follow the newest release under one dotted segment prefix.
      if ("prefetch" in ($entry | columns)) and ($entry.prefetch != "manual") {
        error make {msg: $"($name): unsupported prefetch policy ($entry.prefetch); this updater only handles flat PyPI sdist pins and prefetch=manual holds"}
      }
      if ("prefetch" in ($entry | columns)) and ($entry.prefetch == "manual") {
        print $"(ansi yellow)skipping ($name): prefetch=manual; refresh this platform pin by hand(ansi reset)"
        $entry
      } else if ("hold" in ($entry | columns)) {
        print $"(ansi yellow)skipping ($name): hold=($entry.hold)(ansi reset)"
        $entry
      } else {
        let project = $name
        let metadata = (http get $"https://pypi.org/pypi/($project)/json")
        let version = (
          if "track" in ($entry | columns) {
            tracked-version $name $metadata.releases $entry.track
          } else {
            $metadata.info.version
          }
        )
        let sdists = (
          $metadata.releases
          | get $version
          | where packagetype == "sdist"
        )
        if ($sdists | is-empty) {
          error make {msg: $"($name): PyPI release ($version) has no sdist"}
        }
        let sdist = ($sdists | first)
        let hash = (sri-from-hex $sdist.digests.sha256)
        $entry
        | upsert version $version
        | upsert url (source-url $project $sdist.filename)
        | upsert hash $hash
      }
    }

    def main [] {
      const out = "packages/mcp/pins.json"
      let pins = (open $out)
      let updated = (
        $pins
        | transpose name entry
        | reduce --fold {} {|row acc|
            $acc | insert $row.name (refresh-pin $row.name $row.entry)
          }
      )
      $updated | to json --indent 2 | $in + "\n" | save --force $out
      print $"updated ($out) from PyPI metadata"
    }
  '';
}
