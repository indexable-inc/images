# Default ix dev base image: agent CLIs plus a normal build toolchain.
# The auto-enabled base profile (modules/profiles/base) supplies version
# control, editors, the nushell workspace wrapper, gdb/lldb, strace, tcpdump,
# jq, btop, bpftrace, lsof, ncdu, pv, file, and the gnutar/gzip/zstd trio
# needed to stay `ix up`-switchable.
{
  lib,
  pkgs,
  ...
}:
{
  # The agent CLIs (wrapped Claude Code + Codex) and the managed-settings policy
  # live in one reusable module so the base image and `index.lib.mkDev` cannot
  # drift. `ix.dev.agents.{claude,codex}` default to true, so importing it ships
  # both. See lib/dev/agents.nix for the wrapper and policy rationale.
  imports = [ ../../../lib/dev/agents.nix ];

  ix.image.name = lib.mkDefault "development-base";

  # `pkgs.claude-code` ships under Anthropic's commercial terms (unfree in
  # nixpkgs). The allow-by-name exception lives on the shared image nixpkgs
  # instance (lib/image/default.nix): every image shares ONE instance via
  # `nixpkgs.pkgs`, so a per-image `nixpkgs.config` is ignored here and in fact
  # fails an assertion.

  environment.systemPackages = builtins.attrValues {
    inherit (pkgs)
      # Browser automation for agents. `agent-browser` (vercel-labs) is the CLI
      # surface; `chromium` is the actual browser it drives. agent-browser
      # auto-detects a Chromium binary on PATH so no extra wiring is needed.
      # Kept local-only (no Browserbase / cloud provider) so sandboxes work
      # offline and don't need outbound API keys.
      agent-browser
      chromium

      # Build toolchain. Most ecosystems lean on cmake / make / ninja and
      # pkg-config; rustup keeps the toolchain pinnable per-project rather than
      # locking the image to one rustc.
      cmake
      gcc
      gnumake
      ninja
      pkg-config
      rustup

      # Default language runtimes that show up across most dev sessions.
      nodejs
      python3
      ;
  };
}
