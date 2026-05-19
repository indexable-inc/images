# Default ix dev base image: agent CLIs plus a normal build toolchain.
# The auto-enabled base profile (modules/profiles/base.nix) supplies the
# nushell workspace wrapper, gdb/lldb, strace, tcpdump, jq, btop, bpftrace,
# lsof, ncdu, pv, file, and the gnutar/gzip/zstd trio needed to stay
# `ix switch`-able. Editors and version control are not in base, so they
# live here.
{ lib, pkgs, ... }:
{
  ix.image.name = "development-base";

  # `pkgs.claude-code` ships under Anthropic's commercial terms (unfree in
  # nixpkgs). Allow it by name so the exception is auditable and narrow;
  # codex is Apache-2.0 and does not need a predicate entry. Do not flip
  # `allowUnfree` to `true` globally: every other unfree package would slip
  # into this image unreviewed.
  nixpkgs.config.allowUnfreePredicate = pkg: builtins.elem (lib.getName pkg) [ "claude-code" ];

  environment.systemPackages = builtins.attrValues {
    inherit (pkgs)
      # Coding agents. Both authenticate at first use inside the VM; no
      # API keys are baked into the image per the trust model.
      claude-code
      codex

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
