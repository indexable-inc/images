# Zsh, ported from dots/zsh in the source repo. There the file was a live
# checkout sourced from a nix-store shim; here the same content is declared
# through programs.zsh. The sops secret loading and the dark/light theme
# plumbing (prompt zstyles, ZSH_HIGHLIGHT_STYLES, BAT_THEME flipping) are
# theme/secret machinery and are not ported; BAT_THEME is pinned to the
# source's dark default instead. Tool hooks (zoxide, direnv, fzf) come from
# the respective program modules in ./home.nix rather than hand-written
# `eval "$(... init zsh)"` lines.
{
  config,
  pkgs,
  ...
}: {
  home.packages = [pkgs.pure-prompt];

  programs.zsh = {
    enable = true;

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

      # --- bare `ssh <host>` / `mosh <host>` auto-attach the remote mux ---
      # When the only argument is a plain hostname (no remote command, no
      # flags), run the remote's mux launcher as the remote command: you land
      # in the last nvim session; <c-b>d detaches to a remote shell and
      # exiting that closes the connection. Every other form (`ssh host cmd`,
      # any flag, scp, git) passes through to the real binary untouched.
      # Needs mux in the remote's profile (see ./mux.nix).
      _is_bare_host() {
        [[ $# -eq 1 && "$1" != -* ]]
      }

      ssh() {
        if _is_bare_host "$@"; then
          command ssh -t "$1" mux
        else
          command ssh "$@"
        fi
      }

      mosh() {
        if _is_bare_host "$@"; then
          command mosh "$1" -- mux
        else
          command mosh "$@"
        fi
      }

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

      # --- vi-mode cursor shape: beam for insert, block for command ---
      autoload -Uz add-zle-hook-widget
      _cursor() { printf '\e[%s q' "''${1:-6}"; }
      _cursor_select() { [[ "$KEYMAP" == vicmd ]] && _cursor 2 || _cursor 6; }
      _cursor_beam() { _cursor 6; }
      add-zle-hook-widget zle-keymap-select _cursor_select
      add-zle-hook-widget zle-line-init _cursor_beam
      add-zle-hook-widget zle-line-finish _cursor_beam
      precmd() { _cursor_beam; }
      preexec() { _cursor_beam; }

      # Inside an nvim :terminal (mux windows), report the cwd via OSC 7 so
      # the mux tab bar can rename tabs like tmux automatic-rename. Byte-wise
      # percent encoding (LC_ALL=C) keeps multibyte path names intact.
      _mux_osc7() {
        [[ -n "$NVIM" ]] || return
        local LC_ALL=C ch encoded=""
        for ch in ''${(s::)PWD}; do
          if [[ "$ch" == [A-Za-z0-9/._~-] ]]; then
            encoded+="$ch"
          else
            encoded+="$(printf '%%%02X' "'$ch")"
          fi
        done
        printf '\e]7;file://%s%s\a' "$HOST" "$encoded"
      }
      chpwd_functions+=(_mux_osc7)
      _mux_osc7

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
