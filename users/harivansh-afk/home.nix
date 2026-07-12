# Personal-but-shareable home-manager module for github:harivansh-afk: the
# dotfiles hari runs as the `hari` user on hari-compute-1, ported from his
# personal nix repo (github:harivansh-afk/nix, a mirror of
# git.harivan.sh/harivansh-afk/nix).
#
# Scope is deliberately the server-side daily-driver set: zsh, git, neovim
# (plus mux, the per-project nvim-server multiplexer that replaces tmux),
# and the XDG/session hygiene around them. Everything secret-adjacent in the
# source repo (sops-nix rendering, forgejo credential helpers, tea logins,
# graphite/gcloud token seeding) and everything client-side or darwin-only
# (ghostty, aerospace, karabiner, wallpaper/theme switching, agent CLI
# configs) intentionally stays out. Host/system concerns (accounts, sshd,
# mosh) belong to the consuming host config, as in the source repo.
#
# The user-agnostic machinery lives in shared modules and is only consumed
# here: mux (modules/home/mux.nix), the XDG tidiness set
# (modules/home/xdg-tidy.nix), vi-mode cursor shapes
# (modules/home/zsh-vi-cursor.nix), and the generic CLI package baseline
# (modules/home/cli-baseline.nix). This directory keeps only what is
# genuinely hari's: identity, aliases, prompt taste, his nvim lua tree, and
# per-program settings.
#
# Closed over `ix` for the shared modules/helpers that need it (the mux
# checked-bash launcher, the language-toolchain handles in ./neovim.nix),
# mirroring how users/andrewgazelka/home.nix is wired in flake.nix.
{ix}: {lib, ...}: let
  muxModule = import (ix.paths.modules + "/home/mux.nix") {inherit ix;};
  neovimModule = import ./neovim.nix {inherit ix;};
in {
  imports = [
    (ix.paths.modules + "/home/cli-baseline.nix")
    (ix.paths.modules + "/home/xdg-tidy.nix")
    (ix.paths.modules + "/home/zsh-vi-cursor.nix")
    ./git.nix
    ./shell.nix
    muxModule
    neovimModule
  ];

  # Shared modern-CLI package baseline (bat, delta, eza, fd, ripgrep, ...)
  # instead of restating the generic tool list per-user.
  cliBaseline.enable = true;

  # Tool state/caches out of $HOME (cargo, go, npm/pnpm, python, docker,
  # aws, psql/sqlite histories, wget/less) — shared policy, not hari's.
  xdgTidy.enable = true;

  # Beam cursor in insert mode, block in command mode (vi keymap set in
  # ./shell.nix).
  zshViCursor.enable = true;

  # The per-project nvim-server multiplexer; bare `ssh <host>`/`mosh <host>`
  # auto-attach and OSC 7 cwd reporting ride in via its zsh integration. The
  # server-side lua half lives in his nvim tree (./config/nvim/lua/mux).
  programs.mux.enable = true;

  home = {
    stateVersion = lib.mkDefault "25.11";

    sessionPath = [
      "$HOME/.local/bin"
      "$HOME/.bun/bin"
    ];

    sessionVariables = {
      VISUAL = "nvim";
      MANPAGER = "nvim +Man!";
      NODE_NO_WARNINGS = "1";
      BUN_INSTALL = "$HOME/.bun";
    };
  };

  xdg.enable = true;

  programs = {
    direnv = {
      enable = true;
      nix-direnv.enable = true;
      # Quiet direnv the way the source dots/direnv/direnv.toml does.
      config.global = {
        hide_env_diff = true;
        log_filter = "^$";
        log_format = "-";
      };
    };

    fzf.enable = true;
    zoxide.enable = true;

    btop = {
      enable = true;
      settings = {
        color_theme = "ayu";
        rounded_corners = false;
        theme_background = false;
        vim_keys = true;
      };
    };

    gh = {
      enable = true;
      # gitCredentialHelper (on by default) supplies the
      # `credential.https://github.com` helper the source git config declared
      # by hand.
      settings = {
        git_protocol = "https";
        prompt = "enabled";
        prefer_editor_prompt = "disabled";
        aliases.co = "pr checkout";
      };
    };

    k9s = {
      enable = true;
      views."v1/pods".columns = [
        "NAME"
        "USER:.metadata.labels.handle"
        "STATUS"
        "READY"
        "AGE"
      ];
    };

    lazygit = {
      enable = true;
      # delta comes from the cli-baseline package set above.
      settings.git.pagers = [
        {
          pager = "delta --paging=never";
          colorArg = "always";
        }
      ];
    };
  };
}
