# Host-independent user policy shared by the macOS workstation and NixOS VM.
{
  lib,
  pkgs,
  ...
}: {
  home = {
    stateVersion = "23.11";
    enableNixpkgsReleaseCheck = false;
    sessionPath = ["$HOME/.local/bin"];
    sessionVariables = {
      EDITOR = "nvim";
      VISUAL = "nvim";
      PAGER = lib.mkDefault "less";
    };
    packages = [
      pkgs.bat
      pkgs.curl
      pkgs.delta
      pkgs.difftastic
      pkgs.duf
      pkgs.dust
      pkgs.eza
      pkgs.fd
      pkgs.htop
      pkgs.jq
      pkgs.ripgrep
      pkgs.rsync
      pkgs.tree
      pkgs.unzip
      pkgs.wget
      pkgs.zstd
    ];
  };

  programs.git = {
    enable = true;
    settings.user = {
      name = "Andrew Gazelka";
      email = "andrew.gazelka@gmail.com";
    };
  };
}
