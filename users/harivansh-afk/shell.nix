# Zsh, ported from dots/zsh in the source repo. There the file was a live
# checkout sourced from a nix-store shim; here the same content is declared
# through programs.zsh. The sops secret loading and the dark/light theme
# plumbing (prompt zstyles, ZSH_HIGHLIGHT_STYLES, BAT_THEME flipping) are
# theme/secret machinery and are not ported; BAT_THEME is pinned to the
# source's dark default instead.
#
# The user-agnostic zsh machinery is consumed from shared modules rather
# than written here: tool hooks come from the zoxide/direnv/fzf program
# modules (./home.nix), the ssh/mosh mux auto-attach and OSC 7 cwd
# reporting from programs.mux.zshIntegration (modules/home/mux.nix), and
# the vi-mode cursor shapes from zshViCursor (modules/home/zsh-vi-cursor.nix).
# What stays here is hari's taste: history shape, aliases, the critic git
# wrapper, the pure prompt setup, and his keybindings.
{
  config,
  pkgs,
  ...
}: {
  home.packages = [pkgs.pure-prompt];

  programs.zsh = {
    enable = true;

    # Pinned to the pre-26.05 home-manager default. Leaving it implicit makes
    # the dotfile location a function of home.stateVersion, which this profile
    # only mkDefaults, so a consuming host bumping the state version would
    # silently relocate .zshrc/.zshenv to $XDG_CONFIG_HOME/zsh.
    dotDir = config.home.homeDirectory;

    defaultKeymap = "viins";

    autosuggestion.enable = true;
    syntaxHighlighting.enable = true;

    history = {
      size = 50000;
      save = 50000;
      path = "${config.xdg.stateHome}/zsh_history";
      ignoreDups = true;
      ignoreAllDups = true;
      ignoreSpace = true;
      extended = true;
      append = true;
      share = false;
    };

    completionInit = ''
      autoload -U compinit && compinit -d "''${XDG_STATE_HOME:-$HOME/.local/state}/zcompdump" -u
      zmodload zsh/complist
      zstyle ':completion:*' matcher-list 'm:{a-zA-Z}={A-za-z}'
    '';

    shellAliases = {
      cl = "clear";
      gc = "git commit";
      gd = "git diff";
      gk = "git checkout";
      gp = "git push";
      gpo = "git pull origin";
      gs = "git status";
      lg = "lazygit";
      nim = "nvim .";
      nv = "nvim .";
      eza = "eza --icons=auto --git --group-directories-first --header";
      ls = "eza";
      ll = "eza -l";
      la = "eza -a";
      lt = "eza --tree";
      lla = "eza -la";
    };

    envExtra = ''
      if [[ -f "$HOME/.cargo/env" ]]; then
        . "$HOME/.cargo/env"
      fi
    '';

    initContent = ''
      export BAT_THEME='gruvbox-dark'

      # --- fire-and-forget `critic review` after staging-ish git verbs ---
      git() {
        command git "$@"
        local exit_code=$?
        case "$1" in
          add | stage | reset | checkout)
            if command -v critic >/dev/null 2>&1; then
              (critic review 2>/dev/null &)
            fi
            ;;
        esac
        return $exit_code
      }

      # --- prompt (pure) ---
      fpath+=("${pkgs.pure-prompt}/share/zsh/site-functions")
      autoload -Uz promptinit && promptinit
      export PURE_PROMPT_SYMBOL=$'\xe2\x9d\xaf'
      export PURE_PROMPT_VICMD_SYMBOL=$'\xe2\x9d\xae'
      export PURE_GIT_DIRTY=""
      export PURE_GIT_UP_ARROW="^"
      export PURE_GIT_DOWN_ARROW="v"
      export PURE_GIT_STASH_SYMBOL="="
      export PURE_CMD_MAX_EXEC_TIME=5
      export PURE_GIT_PULL=0
      export PURE_GIT_UNTRACKED_DIRTY=1
      zstyle ':prompt:pure:git:stash' show yes
      typeset -g prompt_newline=' '
      prompt pure

      [ -s "$HOME/.bun/_bun" ] && source "$HOME/.bun/_bun"

      bindkey '^k' forward-char
      bindkey '^j' backward-char
    '';
  };
}
