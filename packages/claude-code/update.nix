# Refreshes every committed artifact that carries the pinned Claude Code
# version, so a bump cannot land half-applied: manifest.json (from Anthropic's
# published per-version manifest, converting its hex checksums to the SRI hashes
# the fetcher pins), env-registry.tsv (from the `envRegistry` derivation), the
# version marker and name count in the Home Manager env reference block, and the
# stock system-prompt snapshots. The slug map lives here as the single owner;
# default.nix only reads it back. The updater fails closed unless the manifest's
# detached GPG signature verifies against the pinned release signing key
# (release-signing-key.asc, fingerprint 31DD DE24 DDFA B679 F42D 7BD2 BAA9 29FF
# 1A7E CACE, published at downloads.claude.ai/keys/claude-code.asc), so a spoofed
# manifest cannot inject hashes for attacker-controlled binaries.
{
  writeNushellApplication,
  nix,
  gnupg,
}:
writeNushellApplication {
  name = "claude-code-update";
  runtimeInputs = [
    nix
    gnupg
  ];
  meta.description = "Refresh the signed Claude Code manifest and every artifact that carries its version";
  text = ''
    # nu
    const base = "https://storage.googleapis.com/claude-code-dist-86c565f3-f756-42ad-8dfa-d59b1c096819/claude-code-releases"
    const signing_key = "${./release-signing-key.asc}"
    const slugs = {
      "aarch64-darwin": "darwin-arm64",
      "x86_64-darwin": "darwin-x64",
      "x86_64-linux": "linux-x64",
      "aarch64-linux": "linux-arm64"
    }

    const registry_tsv = "packages/claude-code/env-registry.tsv"
    const hm_module = "packages/agent/home-manager/claude-code.nix"

    # The two lines in the Home Manager env reference block that carry a value
    # derived from the pinned CLI: `line` locates the line, `field` is the span
    # rewritten inside it. checks.claude-code-knob-reference asserts the version
    # marker directly, so leaving it stale turns main red; nothing asserts the
    # count, which is worse, because it just quietly lies about the TSV.
    const env_version_marker = {
      what: "env reference version",
      line: 'BEGIN claude-code env reference \(extracted from Claude Code cli\.js [^)]+\)',
      field: 'cli\.js [^)]+'
    }
    const env_count_marker = {
      what: "env reference name count",
      line: 'env-registry\.tsv \(all [0-9]+ names',
      field: '\(all [0-9]+ names'
    }

    # Index of the single line in `file` matching `marker.line`. Anything other
    # than exactly one match is fatal and names the file, the marker and the
    # pattern: a reworded marker must stop the updater, never be skipped. A
    # quiet skip is how the 2.1.215 -> 2.1.220 bump shipped a stale reference
    # and only found out one gate later, after it had landed on main.
    def marker_index [file: string, marker: record<what: string, line: string, field: string>] {
      let lines = (open --raw $file | lines)
      let hits = ($lines | enumerate | where {|row| $row.item =~ $marker.line })
      if ($hits | length) != 1 {
        error make {
          msg: ([
            $"claude-code: ($file) has ($hits | length) lines matching the ($marker.what) marker, expected exactly 1."
            $"  pattern: ($marker.line)"
            "  The marker was reworded, moved or removed. Restore its wording, or teach"
            "  the matching marker in packages/claude-code/update.nix the new one -- the"
            "  updater refuses to skip it, because a skipped marker is a stale reference"
            "  that fails checks.claude-code-knob-reference after the bump has landed."
          ] | str join (char newline))
        }
      }
      $hits | first | get index
    }

    # Substitutes `marker.field` on that one line with `value`, leaving the rest
    # of the file byte-identical (so a re-run with the same value is a no-op).
    def rewrite_marker [file: string, marker: record<what: string, line: string, field: string>, value: string] {
      let idx = (marker_index $file $marker)
      let lines = (open --raw $file | lines)
      let rewritten = ($lines | get $idx | str replace --regex $marker.field $value)
      $"($lines | update $idx $rewritten | str join (char newline))(char newline)" | save --force $file
    }

    # Rebuilds env-registry.tsv from the pinned binary and re-stamps the Home
    # Manager env reference block from it. The freshly generated TSV is the
    # single source for both substituted values: its header carries the version
    # the derivation read out of manifest.json, and its body is the name list,
    # so neither the version string nor the count is restated here.
    def refresh_env_registry [] {
      # Fail before the multi-minute build if a marker has drifted, rather than
      # after, leaving a tree with a new manifest and a stale reference.
      for marker in [$env_version_marker $env_count_marker] { marker_index $hm_module $marker | ignore }

      let build = (^nix build --no-link --print-out-paths .#claude-code.envRegistry | complete)
      if $build.exit_code != 0 {
        error make { msg: $"claude-code: failed to build .#claude-code.envRegistry\n($build.stderr)" }
      }
      open --raw ($build.stdout | lines | last) | save --force $registry_tsv
      print $"updated ($registry_tsv)"

      let rows = (open --raw $registry_tsv | lines)
      let stamped = ($rows | first | parse --regex 'cli\.js (?<version>\S+)')
      if ($stamped | is-empty) {
        error make { msg: $"claude-code: ($registry_tsv) header does not name a CLI version: ($rows | first)" }
      }
      let version = ($stamped | first | get version)
      let count = ($rows | where {|row| not ($row | str starts-with "#") } | length)

      rewrite_marker $hm_module $env_version_marker $"cli.js ($version)"
      rewrite_marker $hm_module $env_count_marker ("(all " + ($count | into string) + " names")
      print $"updated ($hm_module) env reference: cli.js ($version), ($count) names"
    }

    def refresh_prompts [] {
      let prompts_dir = "packages/claude-code/system-prompts"
      let models = (open $"($prompts_dir)/models.json")

      $models
      | transpose name model
      | each {|row|
          let capture = (
            ^nix run .#claude-code.extractStockSystemPrompt -- --mode stock --model $row.model --json
            | complete
          )
          if $capture.exit_code != 0 {
            error make { msg: $"claude-code: failed to capture ($row.name) system prompt\n($capture.stderr)" }
          }

          let prompt = (
            $capture.stdout
            | from json
            | get system
            | where {|block| not (($block.text | into string) | str starts-with "x-anthropic-billing-header:") }
            | get text
            | str join "\n"
            | str replace --all --regex "claude-extract-home[-_][A-Za-z0-9_-]+" "claude-extract-home"
            | str replace --all --regex "claude-extract-cwd[-_][A-Za-z0-9_-]+" "claude-extract-cwd"
          )
          let out = $"($prompts_dir)/($row.name).txt"
          $"($prompt)\n" | save --force $out
          print $"updated ($out) from model ($row.model)"
        }
    }

    # The one version-derived artifact deliberately left to a human, because the
    # loud failure is better than the automation: see the note above
    # `devChannelsGateAnchor` in ./default.nix.
    def print_anchor_reminder [] {
      print ""
      print "reminder: dev-channels gate anchors are NOT regenerated by this updater."
      print "  packages/claude-code/default.nix `devChannelsGateAnchor` holds a per-system"
      print "  byte-patch target counted against the pinned binaries. A release that"
      print "  reminifies the surrounding identifiers invalidates it; the byte patcher's"
      print "  `expect = 1` gate then fails the build with `COUNT DRIFT ... expected 1,"
      print "  found 0`. Re-derive the anchor from the new binary when that fires."
      print "  It stays manual on purpose: these are security-relevant patch targets, and"
      print "  a machine rewriting them would turn a loud failure into a silent one."
    }

    # Run from the repo root: `nix run .#claude-code.updateScript -- [version]`.
    # Without a version argument it tracks Anthropic's `latest` pointer.
    # Use --prompts-only to recapture snapshots for the already-pinned package,
    # --registry-only to re-extract the env registry for it, or --skip-prompts
    # when only the signed binary manifest and the env registry should move.
    def main [
      version?: string
      --prompts-only
      --registry-only
      --skip-prompts
    ] {
      if $prompts_only {
        refresh_prompts
        return
      }
      if $registry_only {
        refresh_env_registry
        return
      }

      let v = ($version | default (http get $"($base)/latest" | str trim))

      # Download the exact bytes we verify, then parse the same file.
      let work = (mktemp --directory)
      let manifest_path = $"($work)/manifest.json"
      let sig_path = $"($work)/manifest.json.sig"
      http get --raw $"($base)/($v)/manifest.json" | save --force $manifest_path
      http get --raw $"($base)/($v)/manifest.json.sig" | save --force $sig_path

      # Fail closed: only the pinned key lives in this GNUPGHOME, so a
      # zero exit from --verify proves Anthropic signed these exact bytes.
      let gnupghome = (mktemp --directory)
      with-env { GNUPGHOME: $gnupghome } {
        ^gpg --batch --quiet --import $signing_key
        let check = (do { ^gpg --batch --verify $sig_path $manifest_path } | complete)
        if $check.exit_code != 0 {
          error make { msg: $"claude-code: manifest signature verification failed for ($v)\n($check.stderr)" }
        }
      }

      let upstream = (open $manifest_path)
      let platforms = (
        $slugs
        | transpose system slug
        | reduce --fold {} {|row acc|
            let hex = ($upstream.platforms | get $row.slug | get checksum)
            let sri = (^nix hash convert --hash-algo sha256 --to sri $hex | str trim)
            $acc | insert $row.system { slug: $row.slug, hash: $sri }
          }
      )
      let out = "packages/claude-code/manifest.json"
      { version: $v, platforms: $platforms } | to json --indent 2 | save --force $out
      print $"updated ($out) to ($v); signature verified"

      # Everything downstream of manifest.json reads it back out of the working
      # tree, so the version is substituted from exactly one place.
      refresh_env_registry

      if not $skip_prompts {
        refresh_prompts
      }

      print_anchor_reminder
    }
  '';
}
