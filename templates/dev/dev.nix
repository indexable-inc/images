# Your ix dev environment (RFC 0007).
#
# This is an ordinary NixOS module. Write your environment at the top level the
# way you would any NixOS config; reach for `ix.dev.*` only to describe the
# agents, an optional fleet, and an optional shared login. Edit freely and
# commit it to your own repo - this is the one file you own.
{ pkgs, ... }:
{
  # ---- Your environment (every VM this builds) -------------------------------

  environment.systemPackages = [
    pkgs.ripgrep
    pkgs.jq
  ];

  programs.git.enable = true;

  # ---- Agents ----------------------------------------------------------------
  # Claude Code and Codex are installed by default. Turn one off if you want:
  #
  #   ix.dev.agents.codex = false;

  # ---- Fleet (optional) ------------------------------------------------------
  # With nothing here you get one VM named `dev`. Declare nodes to make it a
  # fleet that comes up with a single `nix run .#up`:
  #
  #   ix.dev.fleet = {
  #     agent.replicas = 3;
  #     builder.dependsOn = [ "agent" ];
  #   };

  # ---- Shared login (optional) -----------------------------------------------
  # Give the whole fleet ONE Claude login over a private SMB volume: the first
  # `claude login` on any node logs in every node, and a new replica needs no
  # extra auth. Off by default. `claude` is shared when enabled; set `ix = true`
  # to also share the ix credentials so a node can spawn more VMs.
  #
  #   ix.dev.shared = {
  #     enable = true;
  #     ix = true;
  #     # excludeNodes = [ "builder" ];   # opt specific nodes out
  #   };

  # See the full option reference: `ix.dev.*` in lib/dev/options.nix (RFC 0007).
}
