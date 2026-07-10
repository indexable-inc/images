# mux: hari's per-project neovim-server multiplexer (his tmux replacement),
# ported from scripts/bin/mux.sh + lua/mux in the source repo. Each project
# (git root) gets one `nvim --headless --listen <socket>` server running the
# real config with lua/mux activated; clients attach with `--remote-ui`, and
# bare `ssh <host>`/`mosh <host>` from ./shell.nix lands in it.
#
# The script resolves `nvim` from the ambient PATH on purpose: it must attach
# to the profile's wrapped neovim (the one carrying ./config/nvim), not a
# pinned store copy.
#
# Closed over `ix` for the checked-bash writer (lib/util/writers.nix): the
# launcher is 1100 lines of load-bearing process control, native bash
# territory, so it uses the shared escape hatch and gets `bash -n` +
# shellcheck in the build.
{ix}: {
  config,
  lib,
  pkgs,
  ...
}: let
  cfg = config.users.harivansh-afk.mux;

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
      (builtins.readFile ./scripts/mux.sh);
  };
in {
  options.users.harivansh-afk.mux = {
    remotes = lib.mkOption {
      type = lib.types.attrsOf lib.types.str;
      default = {};
      example = {
        hari1 = "hari-compute-1";
      };
      description = ''
        Remote mux catalog as `name -> ssh host` pairs, baked into the
        launcher for `mux list --all` / cross-host project switching. The
        `MUX_REMOTES_FILE` environment variable overrides it at runtime. In
        the source repo the same list came from lib/remotes.nix.
      '';
    };
  };

  config = {
    home.packages = [mux];
  };
}
