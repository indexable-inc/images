# `whence <path|pname>`: deployed config file or installed package ->
# defining nix source line (#2416, #3942).
#
# Reads the provenance manifest that modules/home/provenance.nix and
# modules/darwin/provenance.nix bake into each generation (deployed path ->
# { file, line, rev, drv, source, definitions, settings } under `files`,
# pname -> { file, line, rev, version, definitions } under `packages`), so
# the answer comes from the live profile with zero eval. The argument is
# sniffed, not flagged: an existing path is looked up as a file; anything
# else tries the package namespace first, then the file keys. A query no
# manifest knows about falls back to `nix-store -q --deriver` on the
# resolved store path.
{writeNushellApplication}:
writeNushellApplication {
  name = "whence";
  meta = {
    description = "Deployed config file or installed package -> defining nix source line, from the generation's provenance manifest";
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

    # Manifests of the live generations: the home-manager profile's (XDG
    # location, plus the pre-XDG per-user profile older installs still use)
    # and, on darwin, the running system's.
    def manifests [] {
      let state_home = ($env.XDG_STATE_HOME? | default ($env.HOME | path join ".local" "state"))
      [
        ($state_home | path join "nix" "profiles" "home-manager" "provenance.json")
        $"/nix/var/nix/profiles/per-user/($env.USER)/home-manager/provenance.json"
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

    # A package entry prints like a file entry, titled by pname (plus
    # version when the manifest recorded one); print-entry's optional
    # accessors skip the file-only fields a package entry lacks.
    def print-package [name: string, entry: record] {
      let version = ($entry.version? | default null)
      let title = if $version == null { $name } else { $"($name) ($version)" }
      print-entry $title $entry
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

    def main [target: string] {
      # A trailing slash on HOME would make the ($home)/ prefix tests below
      # miss every home file.
      let home = ($env.HOME | str trim --right --char '/')
      # Logical absolute path (no symlink resolution): manifest keys are
      # deployment targets, which are themselves symlinks into the store.
      let logical = ($target | path expand --no-symlink)

      # An argument that is not an existing path is sniffed as a package
      # name first (#3942); an existing path always means the file, so a
      # package named like a file in the cwd loses to the file.
      if not ($logical | path exists) {
        for manifest_path in (manifests) {
          let packages = (open $manifest_path | get packages? | default {} | transpose key entry)
          let hit = ($packages | where {|row| $row.key == $target })
          if ($hit | is-not-empty) {
            let row = ($hit | first)
            print-package $row.key $row.entry
            return
          }
        }
      }
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
