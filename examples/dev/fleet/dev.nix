# A forkable ix environment (RFC 0007). This is an ordinary NixOS module: write
# your environment at the top level, and use `ix.dev.*` for the fleet and the
# shared volume. After `ix init` this is the one file you edit.
{ pkgs, ... }:
{
  # Your environment: applied to every VM (single or fleet).
  environment.systemPackages = [
    pkgs.ripgrep
    pkgs.jq
  ];
  programs.git.enable = true;

  # Claude Code + Codex are installed by default; toggle either off here.
  # ix.dev.agents.codex = false;

  # A fleet: two interchangeable agents plus a builder that opts out of the
  # shared volume below. Drop `ix.dev.fleet` entirely for a single VM.
  ix.dev.fleet = {
    agent.replicas = 2;
    builder.dependsOn = [ "agent" ];
  };

  # One shared Claude (and ix) login for the fleet, over an SMB volume. The
  # first `claude login` on any agent logs in every agent; `builder` opts out.
  ix.dev.shared = {
    enable = true;
    ix = true; # also share ~/.n so a node can spawn more VMs (claude is shared by default)
    excludeNodes = [ "builder" ];
  };
}
