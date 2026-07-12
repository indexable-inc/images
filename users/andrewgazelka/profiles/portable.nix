# Host-independent user policy shared by the macOS workstation and NixOS VM.
{lib, ...}: {
  # Shared modern-CLI tool set (bat, delta, eza, fd, ripgrep, ...); the list
  # itself is general and lives in modules/home/cli-baseline.nix.
  imports = [../../../modules/home/cli-baseline.nix];
  cliBaseline.enable = true;

  home = {
    stateVersion = "23.11";
    enableNixpkgsReleaseCheck = false;
    sessionPath = ["$HOME/.local/bin"];
    sessionVariables = {
      EDITOR = "nvim";
      VISUAL = "nvim";
      PAGER = lib.mkDefault "less";
    };
  };

  programs.git = {
    enable = true;
    settings.user = {
      name = "Andrew Gazelka";
      email = "andrew.gazelka@gmail.com";
    };
  };
}
