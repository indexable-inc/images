# Refreshes manifest.json from Anthropic's published per-version manifest,
# converting its hex checksums to the SRI hashes the fetcher pins, then refreshes
# the committed stock system-prompt snapshots. The slug map lives here as the
# single owner; default.nix only reads it back. The updater fails closed unless
# the manifest's detached GPG signature verifies against the pinned release
# signing key (release-signing-key.asc, fingerprint 31DD DE24 DDFA B679 F42D
# 7BD2 BAA9 29FF 1A7E CACE, published at downloads.claude.ai/keys/claude-code.asc),
# so a spoofed manifest cannot inject hashes for attacker-controlled binaries.
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
  meta.description = "Refresh the signed Claude Code manifest and stock system-prompt snapshots";
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

    def refresh_prompts [] {
      let prompts_dir = "packages/agent/claude-code/system-prompts"
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

    # Run from the repo root: `nix run .#claude-code.updateScript -- [version]`.
    # Without a version argument it tracks Anthropic's `latest` pointer.
    # Use --prompts-only to recapture snapshots for the already-pinned package,
    # or --skip-prompts when only the signed binary manifest should move.
    def main [
      version?: string
      --prompts-only
      --skip-prompts
    ] {
      if $prompts_only {
        refresh_prompts
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
      let out = "packages/agent/claude-code/manifest.json"
      { version: $v, platforms: $platforms } | to json --indent 2 | save --force $out
      print $"updated ($out) to ($v); signature verified"

      if not $skip_prompts {
        refresh_prompts
      }
    }
  '';
}
