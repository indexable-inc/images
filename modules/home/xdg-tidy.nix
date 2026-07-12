# XDG hygiene for tools that default to littering $HOME, hoisted out of
# users/harivansh-afk (ported there from github:harivansh-afk/nix) because
# none of it is per-user policy: every entry just points a tool's state,
# cache, or config at the XDG base directories. Option-gated like the other
# shared home modules: import it and set `xdgTidy.enable = true`. Personal
# session variables belong in the consumer's own `home.sessionVariables`,
# which home-manager merges with these.
#
# The npmrc/pythonrc/wgetrc companions are the minimal config files needed
# for the redirects that cannot be expressed as environment variables alone.
{
  config,
  lib,
  ...
}: let
  cfg = config.xdgTidy;
in {
  options.xdgTidy = {
    enable = lib.mkEnableOption "the XDG tidiness set (tool state/caches out of $HOME)";
  };

  config = lib.mkIf cfg.enable {
    home = {
      sessionPath = [
        "${config.xdg.dataHome}/cargo/bin"
        "${config.xdg.dataHome}/go/bin"
        "${config.xdg.dataHome}/npm/bin"
        "${config.xdg.dataHome}/pnpm"
      ];

      sessionVariables = {
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

    xdg.configFile = {
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
}
