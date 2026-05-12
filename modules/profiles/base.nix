# Base CLI profile.
#
# Auto-enabled by `lib/ix-base.nix` so every image gets a usable shell out of
# the box. Images that want a minimal closure can opt out with
# `ix.profiles.base.enable = false;`.
{
  config,
  lib,
  pkgs,
  ...
}:
{
  options.ix.profiles.base.enable =
    lib.mkEnableOption "base CLI tools (editors, shells, network, files, debug, misc)";

  config = lib.mkIf config.ix.profiles.base.enable {
    environment = {
      systemPackages = builtins.attrValues {
        # AI
        # TODO: re-enable once binary cache is available. These come from custom
        # flakes (claude-code-nix, codex-cli-nix) and build from source.
        # inherit (pkgs.llm-agents) claude-code codex;

        # editors
        inherit (pkgs) neovim;

        # shells
        inherit (pkgs) nushell fish zsh;

        # net
        inherit (pkgs)
          curl
          wget
          openssh
          iproute2
          dnsutils
          ;

        # files
        inherit (pkgs)
          bzip2
          fd
          file
          gzip
          gnutar
          tree
          unzip
          xz
          zstd
          ripgrep
          less
          jq
          ;

        # remote workspaces
        # TODO: re-enable tonbo-artifacts (custom Rust build, no cache hits).
        inherit (pkgs) fuse3;

        # debug
        inherit (pkgs)
          btop
          htop
          strace
          lsof
          gdb
          procps
          tcpdump
          perf
          ;

        # misc
        inherit (pkgs)
          tmux
          zellij
          git
          bat
          eza
          zoxide
          fzf
          delta
          dust
          duf
          hyperfine
          tokei
          ;
      };
      variables.EDITOR = "nvim";
    };

    programs = {
      bash.completion.enable = true;
      zsh.enable = true;
      fish.enable = true;
    };
  };
}
