# Shared modern-CLI package baseline for home-manager configurations.
#
# The set of quality-of-life CLI tools everyone reaches for (better cat/ls/du,
# ripgrep, jq, ...) is not per-user policy, so it lives here instead of being
# copy-pasted into each user's profile. Option-gated like the other shared
# home modules: import the module wherever it might be wanted and flip
# `cliBaseline.enable = true` where it is.
#
# `cliBaseline.packages` is the whole list and can be overridden (not merged)
# to drop or swap tools; personal additions belong in the consumer's own
# `home.packages`, which home-manager concatenates with this one.
{
  config,
  lib,
  pkgs,
  ...
}: let
  cfg = config.cliBaseline;
in {
  options.cliBaseline = {
    enable = lib.mkEnableOption "the shared modern-CLI package baseline";

    packages = lib.mkOption {
      type = lib.types.listOf lib.types.package;
      default = [
        pkgs.bat # cat with syntax highlighting
        pkgs.curl
        pkgs.delta # syntax-highlighting git pager
        pkgs.difftastic # structural (AST) diff
        pkgs.duf # df with readable output
        pkgs.dust # du as a tree, biggest first
        pkgs.eza # modern ls
        pkgs.fd # modern find
        pkgs.htop
        pkgs.jq
        pkgs.ripgrep
        pkgs.rsync
        pkgs.tree
        pkgs.unzip
        pkgs.wget
        pkgs.zstd
      ];
      defaultText = lib.literalExpression "the standard modern-CLI tool set (bat, delta, eza, fd, ripgrep, ...)";
      description = "Packages installed by the baseline; override to trim or swap tools.";
    };
  };

  config = lib.mkIf cfg.enable {
    home.packages = cfg.packages;
  };
}
