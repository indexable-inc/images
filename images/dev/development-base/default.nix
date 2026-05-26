# Default ix dev base image: agent CLIs plus a normal build toolchain.
# The auto-enabled base profile (modules/profiles/base.nix) supplies version
# control, editors, the nushell workspace wrapper, gdb/lldb, strace, tcpdump,
# jq, btop, bpftrace, lsof, ncdu, pv, file, and the gnutar/gzip/zstd trio
# needed to stay `ix switch`-able.
{ lib, pkgs, ... }:
{
  ix.image.name = "development-base";

  # `pkgs.claude-code` ships under Anthropic's commercial terms (unfree in
  # nixpkgs). Allow it by name so the exception is auditable and narrow;
  # codex is Apache-2.0 and does not need a predicate entry. Do not flip
  # `allowUnfree` to `true` globally: every other unfree package would slip
  # into this image unreviewed.
  nixpkgs.config.allowUnfreePredicate =
    pkg: builtins.elem (pkg.pname or (lib.getName pkg)) [ "claude-code" ];

  environment.systemPackages = builtins.attrValues {
    inherit (pkgs)
      # Coding agents. Both authenticate at first use inside the VM; no
      # API keys are baked into the image per the trust model.
      claude-code
      codex

      # Browser automation for agents. `agent-browser` (vercel-labs) is the
      # CLI surface; `chromium` is the actual browser it drives. agent-browser
      # auto-detects a Chromium binary on PATH so no extra wiring is needed.
      # Kept local-only (no Browserbase / cloud provider) so sandboxes work
      # offline and don't need outbound API keys.
      agent-browser
      chromium

      # Build toolchain. Most ecosystems lean on cmake / make / ninja and
      # pkg-config; rustup keeps the toolchain pinnable per-project rather
      # than locking the image to one rustc.
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
