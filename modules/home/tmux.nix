{
  config,
  lib,
  pkgs,
  ...
}: let
  cfg = config.programs.tmux.structured;
  types = lib.types;
  renderValue = value: lib.escapeShellArg value;
  renderSettings = command: settings:
    lib.concatMap (name:
      map (value: "${command} ${name} ${renderValue value}")
      (
        if builtins.isList settings.${name}
        then settings.${name}
        else [settings.${name}]
      ))
    (builtins.attrNames settings);
  rendered = lib.concatLines (
    (renderSettings "set -g" cfg.set)
    ++ (renderSettings "set -s" cfg.server)
    ++ (renderSettings "set -as" cfg.appendServer)
    ++ (renderSettings "setw -g" cfg.window)
    ++ (map (plugin: "set -g @plugin ${renderValue plugin}") cfg.plugins)
    ++ (renderSettings "set -g" (lib.mapAttrs' (name: value: lib.nameValuePair "@${name}" value) cfg.pluginSettings))
    ++ (map (binding: "bind${lib.optionalString binding.noPrefix " -n"}${lib.optionalString (binding.table != null) " -T ${binding.table}"} ${binding.key} ${binding.command}") cfg.bindings)
    ++ (map (key: "unbind -n ${key}") cfg.unbindNoPrefix)
    ++ (map (command: "run ${renderValue command}") cfg.run)
    ++ cfg.extra
  );
in {
  options.programs.tmux.structured = {
    enable = lib.mkEnableOption "a declarative tmux.conf";
    set = lib.mkOption {
      type = types.attrsOf (types.either types.str (types.listOf types.str));
      default = {};
      description = "Global tmux options, rendered as `set -g` directives.";
    };
    server = lib.mkOption {
      type = types.attrsOf (types.either types.str (types.listOf types.str));
      default = {};
      description = "Server tmux options, rendered as `set -s` directives.";
    };
    appendServer = lib.mkOption {
      type = types.attrsOf (types.listOf types.str);
      default = {};
      description = "Server options appended with `set -as`, such as terminal features.";
    };
    window = lib.mkOption {
      type = types.attrsOf (types.either types.str (types.listOf types.str));
      default = {};
      description = "Window tmux options, rendered as `setw -g` directives.";
    };
    plugins = lib.mkOption {
      type = types.listOf types.str;
      default = [];
      description = "TPM plugin identifiers.";
    };
    pluginSettings = lib.mkOption {
      type = types.attrsOf (types.either types.str (types.listOf types.str));
      default = {};
      description = "TPM plugin settings without the leading `@`.";
    };
    bindings = lib.mkOption {
      type = types.listOf (types.submodule {
        options = {
          key = lib.mkOption {type = types.str;};
          command = lib.mkOption {type = types.str;};
          noPrefix = lib.mkOption {
            type = types.bool;
            default = false;
          };
          table = lib.mkOption {
            type = types.nullOr types.str;
            default = null;
          };
        };
      });
      default = [];
      description = "Tmux key bindings.";
    };
    unbindNoPrefix = lib.mkOption {
      type = types.listOf types.str;
      default = [];
      description = "Keys to unbind with `unbind -n`.";
    };
    run = lib.mkOption {
      type = types.listOf types.str;
      default = [];
      description = "Tmux scripts to run after all settings are loaded.";
    };
    extra = lib.mkOption {
      type = types.listOf types.str;
      default = [];
      description = "Ordered tmux directives that have no structured representation.";
    };
  };

  config = lib.mkIf cfg.enable {
    home.file.".tmux.conf".source = pkgs.writeText "tmux.conf" rendered;
  };
}
