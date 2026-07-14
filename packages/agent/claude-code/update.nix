# Refreshes manifest.json from Anthropic's published per-version manifest,
# converting its hex checksums to the SRI hashes the fetcher pins, then
# refreshes the committed stock system-prompt snapshots. The slug map lives
# here as the single owner; default.nix only reads it back. The behavior lives
# in the pin-update engine's `claude-code` mode (packages/nix/pin-update);
# this file only renders the spec. The updater fails closed unless the
# manifest's detached GPG signature verifies against the pinned release
# signing key (release-signing-key.asc, fingerprint 31DD DE24 DDFA B679 F42D
# 7BD2 BAA9 29FF 1A7E CACE, published at downloads.claude.ai/keys/claude-code.asc),
# so a spoofed manifest cannot inject hashes for attacker-controlled binaries.
#
# Run from the repo root: `nix run .#claude-code.updateScript -- [version]`.
# Without a version argument it tracks Anthropic's `latest` pointer. Use
# --prompts-only to recapture snapshots for the already-pinned package, or
# --skip-prompts when only the signed binary manifest should move.
{
  pinUpdate,
  nix,
  gnupg,
}:
pinUpdate.mkUpdateScript {
  name = "claude-code-update";
  description = "Refresh the signed Claude Code manifest and stock system-prompt snapshots";
  # nix for the prompt recapture (`nix run .#claude-code.extractStockSystemPrompt`);
  # gnupg for the fail-closed signature check.
  runtimeInputs = [
    nix
    gnupg
  ];
  spec = {
    mode = "claude-code";
    base = "https://storage.googleapis.com/claude-code-dist-86c565f3-f756-42ad-8dfa-d59b1c096819/claude-code-releases";
    signingKey = "${./release-signing-key.asc}";
    # A list, not an attrset: `builtins.toJSON` sorts attrset keys, and the
    # rewritten manifest.json keeps its platforms in this order.
    platforms = [
      {
        system = "aarch64-darwin";
        slug = "darwin-arm64";
      }
      {
        system = "x86_64-darwin";
        slug = "darwin-x64";
      }
      {
        system = "x86_64-linux";
        slug = "linux-x64";
      }
      {
        system = "aarch64-linux";
        slug = "linux-arm64";
      }
    ];
    manifest = "packages/agent/claude-code/manifest.json";
    prompts = "packages/agent/claude-code/system-prompts";
  };
}
