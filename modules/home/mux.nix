# mux: a per-project Neovim-server multiplexer (tmux replacement), hoisted
# out of users/harivansh-afk (ported there from github:harivansh-afk/nix)
# because nothing in the launcher is user-specific. Each project (git root)
# gets one long-lived `nvim --headless --listen <socket>` server; `mux`
# attaches with `--remote-ui`, so sessions survive disconnects like tmux
# while the multiplexer itself is nvim. `mux list --all` federates project
# listings across the configured remotes over ssh.
#
# Option-gated like the other shared home modules: import it and set
# `programs.mux.enable = true`.
#
# Contract: the launcher resolves `nvim` from the ambient PATH on purpose
# (it must attach to the profile's wrapped neovim carrying the user's
# config, not a pinned store copy), and it activates a user-supplied `mux`
# lua module on the server side (`require("mux").setup()`, pcall-guarded,
# with MUX=1 exported) — bring an nvim config that ships one, e.g.
# users/harivansh-afk/config/nvim/lua/mux.
#
# Closed over `ix` for the checked-bash writer (lib/util/writers.nix): the
# launcher is ~1100 lines of load-bearing POSIX process control, native
# bash territory, so it uses the shared escape hatch and gets `bash -n` +
# shellcheck in the build.
{ix}: {
  config,
  lib,
  pkgs,
  ...
}: let
  cfg = config.programs.mux;

  remotesText = lib.concatStrings (
    lib.mapAttrsToList (name: host: "${name} ${host}\n") cfg.remotes
  );

  mux = ix.writeBashApplication pkgs {
    name = "mux";
    runtimeInputs =
      [
        pkgs.coreutils
        pkgs.fzf
        pkgs.gawk
        pkgs.git
        pkgs.gnugrep
        pkgs.gnused
        pkgs.openssh
      ]
      ++ lib.optional pkgs.stdenv.hostPlatform.isLinux pkgs.util-linux;
    text =
      builtins.replaceStrings ["@MUX_REMOTES@"] [remotesText]
      (builtins.readFile ./mux/mux.sh);
    meta.description = "Per-project nvim-server multiplexer (tmux replacement)";
  };
in {
  options.programs.mux = {
    enable = lib.mkEnableOption "mux, the per-project nvim-server multiplexer";

    remotes = lib.mkOption {
      type = lib.types.attrsOf lib.types.str;
      default = {};
      example = {
        hari1 = "hari-compute-1";
      };
      description = ''
        Remote mux catalog as `name -> ssh host` pairs, baked into the
        launcher for `mux list --all` / cross-host project switching. The
        `MUX_REMOTES_FILE` environment variable overrides it at runtime.
      '';
    };

    zshIntegration = lib.mkOption {
      type = lib.types.bool;
      default = true;
      description = ''
        Add the mux shell glue to zsh: bare `ssh <host>` / `mosh <host>`
        (single plain-hostname argument, no flags, no remote command) run the
        remote's mux launcher as the remote command so you land in its last
        nvim session, and inside an nvim :terminal the cwd is reported via
        OSC 7 so the mux tab bar can rename tabs like tmux automatic-rename.
      '';
    };
  };

  config = lib.mkIf cfg.enable {
    home.packages = [mux];

    programs.zsh.initContent = lib.mkIf cfg.zshIntegration ''
      # --- mux: bare `ssh <host>` / `mosh <host>` auto-attach the remote mux ---
      # When the only argument is a plain hostname, run the remote's mux
      # launcher as the remote command: you land in the last nvim session;
      # <c-b>d detaches to a remote shell and exiting that closes the
      # connection. Every other form (`ssh host cmd`, any flag, scp, git)
      # passes through to the real binary untouched. Needs mux in the
      # remote's profile.
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
    '';
  };
}
