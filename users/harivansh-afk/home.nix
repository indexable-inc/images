# Personal-but-shareable home-manager module for github:harivansh-afk: the
# dotfiles hari runs as the `hari` user on hari-compute-1, ported from his
# personal nix repo (github:harivansh-afk/nix, a mirror of
# git.harivan.sh/harivansh-afk/nix).
#
# Scope is deliberately the server-side daily-driver set: zsh, git, neovim
# (plus `mux`, the per-project nvim-server multiplexer that replaces tmux),
# and the XDG/session hygiene around them. Everything secret-adjacent in the
# source repo (sops-nix rendering, forgejo credential helpers, tea logins,
# graphite/gcloud token seeding) and everything client-side or darwin-only
# (ghostty, aerospace, karabiner, wallpaper/theme switching, agent CLI
# configs) intentionally stays out. Host/system concerns (accounts, sshd,
# mosh) belong to the consuming host config, as in the source repo.
#
# Closed over `ix` for the checked-bash writer that packages mux and the
# shared language-toolchain handles (lib/util/writers.nix,
# lib/languages/), mirroring how users/andrewgazelka/home.nix is wired in
# flake.nix.
{ix}: {
  config,
  lib,
  pkgs,
  ...
}: let
  muxModule = import ./mux.nix {inherit ix;};
  neovimModule = import ./neovim.nix {inherit ix;};
in {
  imports = [
    ../../modules/home/cli-baseline.nix
    ./git.nix
    ./shell.nix
    muxModule
    neovimModule
  ];

  # Shared modern-CLI package baseline (bat, delta, eza, fd, ripgrep, ...)
  # instead of restating the generic tool list per-user; hari-specific tools
  # ride below and in the sibling modules.
  cliBaseline.enable = true;

  home = {
    stateVersion = lib.mkDefault "25.11";

    packages = [
      # git pager configured in ./git.nix; also used interactively.
      pkgs.diff-so-fancy
    ];

    sessionPath = [
      "$HOME/.local/bin"
      "${config.xdg.dataHome}/cargo/bin"
      "${config.xdg.dataHome}/go/bin"
      "${config.xdg.dataHome}/npm/bin"
      "${config.xdg.dataHome}/pnpm"
      "$HOME/.bun/bin"
    ];

    # The XDG tidiness set from the source repo's session environment
    # (modules/users/user-config/env.nix there): keep tool state out of $HOME.
    sessionVariables = {
      VISUAL = "nvim";
      MANPAGER = "nvim +Man!";
      NODE_NO_WARNINGS = "1";

      LESSHISTFILE = "-";
      WGETRC = "${config.xdg.configHome}/wgetrc";

      CARGO_HOME = "${config.xdg.dataHome}/cargo";
      RUSTUP_HOME = "${config.xdg.dataHome}/rustup";

      GOPATH = "${config.xdg.dataHome}/go";
      GOMODCACHE = "${config.xdg.cacheHome}/go/mod";

      NPM_CONFIG_USERCONFIG = "${config.xdg.configHome}/npm/npmrc";
      NODE_REPL_HISTORY = "${config.xdg.stateHome}/node_repl_history";
      PNPM_HOME = "${config.xdg.dataHome}/pnpm";
      PNPM_NO_UPDATE_NOTIFIER = "true";
      BUN_INSTALL = "$HOME/.bun";

      PYTHONSTARTUP = "${config.xdg.configHome}/python/pythonrc";
      PYTHON_HISTORY = "${config.xdg.stateHome}/python_history";
      PYTHONPYCACHEPREFIX = "${config.xdg.cacheHome}/python";
      PYTHONUSERBASE = "${config.xdg.dataHome}/python";

      DOCKER_CONFIG = "${config.xdg.configHome}/docker";

      AWS_SHARED_CREDENTIALS_FILE = "${config.xdg.configHome}/aws/credentials";
      AWS_CONFIG_FILE = "${config.xdg.configHome}/aws/config";

      PSQL_HISTORY = "${config.xdg.stateHome}/psql_history";
      SQLITE_HISTORY = "${config.xdg.stateHome}/sqlite_history";
    };
  };

  xdg = {
    enable = true;

    configFile = {
      # npm expands ''${XDG_*} itself at runtime; the literals are intended.
      "npm/npmrc".text = ''
        prefix=''${XDG_DATA_HOME}/npm
        cache=''${XDG_CACHE_HOME}/npm
      '';

      "python/pythonrc".text = ''
        # python
        import atexit
        import os
        import readline

        history = os.path.join(os.environ.get('XDG_STATE_HOME', os.path.expanduser('~/.local/state')), 'python_history')

        try:
            readline.read_history_file(history)
        except OSError:
            pass

        def write_history():
            try:
                readline.write_history_file(history)
            except OSError:
                pass

        atexit.register(write_history)
      '';

      "wgetrc".text = ''
        hsts_file = ~/.local/state/wget-hsts
      '';
    };
  };

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
