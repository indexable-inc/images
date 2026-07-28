# A forkable ix environment (RFC 0007). This is an ordinary NixOS module:
# write your environment at the top level, and reach for `ix.dev.*` only to
# configure the agents. After `ix init` this is the one file you edit.
{pkgs, ...}: {
  # Your environment.
  environment.systemPackages = [
    pkgs.ripgrep
    pkgs.jq
  ];
  programs.git.enable = true;

  # Claude Code + Codex are installed by default; toggle either off here.
  # ix.dev.agents.codex = false;
}
