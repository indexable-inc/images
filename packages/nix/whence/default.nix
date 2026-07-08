# `whence <path>`: deployed config file -> defining nix source line (#2416).
#
# Reads the provenance manifest that modules/home/provenance.nix and
# modules/darwin/provenance.nix bake into each generation (deployed path ->
# { file, line, rev, drv, source, definitions, settings }), so the answer
# comes from the live profile with zero eval. A path no manifest knows about
# falls back to `nix-store -q --deriver` on the resolved store path.
{writeNushellApplication}:
writeNushellApplication {
  name = "whence";
  meta = {
    description = "Deployed config file -> defining nix source line, from the generation's provenance manifest";
    mainProgram = "whence";
  };
  # No pinned nix in runtimeInputs: the fallback `nix-store -q --deriver`
  # must speak the host daemon's protocol/experimental-feature set, so it
  # uses the ambient nix, same as push-cache and the updaters.
  text = ''
    # nu

    # Definition sites are store paths of the flake copy
    # (/nix/store/<hash>-source/...); strip the copy prefix so sites print
    # repo-relative.
    def clean-site [file: string] {
      $file | str replace -r '^/nix/store/[a-z0-9]{32}-[^/]+/' ""
    }

    def format-site [site: record] {
      let line = $site.line?
      if $line == null {
        clean-site $site.file
      } else {
        $"(clean-site $site.file):($line)"
      }
    }

    # Manifests of the live generations: the home-manager profile's and, on
    # darwin, the running system's.
    def manifests [] {
      let state_home = ($env.XDG_STATE_HOME? | default ($env.HOME | path join ".local" "state"))
      [
        ($state_home | path join "nix" "profiles" "home-manager" "provenance.json")
        "/run/current-system/provenance.json"
      ] | where {|it| $it | path exists }
    }

    def print-entry [path: string, entry: record] {
      let rev = ($entry.rev? | default "unknown rev")
      let file = ($entry.file? | default "?")
      let line = ($entry.line? | default "?")
      print $"($path)"
      print $"  (clean-site $file):($line) @ ($rev)"
      let sites = ($entry.definitions? | default [])
      if ($sites | length) > 1 {
        print "  defined via:"
        for site in $sites {
          print $"    (format-site $site)"
        }
      }
      for chain in ($entry.settings? | default []) {
        print $"  ($chain.option):"
        for site in ($chain.definitions? | default []) {
          print $"    (format-site $site)"
        }
      }
      if ($entry.source? | default null) != null {
        print $"  source: ($entry.source)"
      }
      if ($entry.drv? | default null) != null {
        print $"  drv: ($entry.drv)"
      }
    }

    # Unmanifested store path: the store's own deriver link is the only
    # provenance left.
    def fallback [resolved: string] {
      print $"no provenance manifest entry for ($resolved)"
      let deriver = (do { ^nix-store -q --deriver $resolved } | complete)
      let out = ($deriver.stdout | str trim)
      if $deriver.exit_code == 0 and $out != "" and $out != "unknown-deriver" {
        print $"  deriver: ($out)"
      } else {
        print "  no deriver recorded either (not built locally, or not a store path)"
        exit 1
      }
    }

    def main [path: string] {
      let home = $env.HOME
      # Logical absolute path (no symlink resolution): manifest keys are
      # deployment targets, which are themselves symlinks into the store.
      let logical = ($path | path expand --no-symlink)
      # Fully resolved payload, for matching by store path and the fallback.
      let resolved = (if ($logical | path exists) { $logical | path expand } else { $logical })

      # Home-manager keys are $HOME-relative, system keys absolute.
      let keys = (
        [$logical $resolved]
        | each {|it|
            if ($it | str starts-with $"($home)/") {
              [$it ($it | str replace $"($home)/" "")]
            } else {
              [$it]
            }
          }
        | flatten
        | uniq
      )

      for manifest_path in (manifests) {
        let files = (open $manifest_path | get files? | default {} | transpose key entry)
        let direct = ($files | where {|row| $row.key in $keys })
        if ($direct | is-not-empty) {
          let row = ($direct | first)
          print-entry $row.key $row.entry
          return
        }
        # No key match: the argument may be the store payload itself, or a
        # file inside a directory-valued source.
        let by_source = ($files | where {|row|
          let src = ($row.entry.source? | default null)
          $src != null and ($resolved == $src or ($resolved | str starts-with $"($src)/"))
        })
        if ($by_source | is-not-empty) {
          let row = ($by_source | first)
          print-entry $row.key $row.entry
          return
        }
      }

      fallback $resolved
    }
  '';
}
