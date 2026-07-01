{ indexPackages }:
{
  config,
  lib,
  pkgs,
  ...
}:
let
  cfg = config.programs.claude-code;
  jsonFormat = pkgs.formats.json { };
  pathLike = lib.types.either lib.types.path lib.types.str;
  indexPkgs = indexPackages pkgs.stdenv.hostPlatform.system;
  systemPromptSource = lib.types.enum [
    "house"
    "stock"
    "text"
  ];

  optionalOverride =
    condition: name: value:
    lib.optionalAttrs condition { ${name} = value; };
  packageOverrides = {
    inherit (cfg)
      addDirs
      dangerouslySkipPermissions
      personalStartupContext
      pluginDirs
      primaryCheckouts
      ;
    omitRules = cfg.systemPrompt.omitRules;
    extraSettings = cfg.defaults;
  }
  // optionalOverride (cfg.defaultMcpServers != null) "mcpServers" cfg.defaultMcpServers
  // optionalOverride (cfg.systemPrompt.source == "text") "systemPrompt" cfg.systemPrompt.text
  // optionalOverride (cfg.systemPrompt.source == "stock") "systemPrompt" null;
  defaultedPackage = cfg.basePackage.override packageOverrides;
in
{
  options.programs.claude-code = {
    basePackage = lib.mkOption {
      type = lib.types.package;
      default = indexPkgs.claude-code;
      defaultText = lib.literalExpression "inputs.index.packages.\${pkgs.stdenv.hostPlatform.system}.claude-code";
      description = "Base index Claude Code wrapper package before Home Manager applies defaults.";
    };

    defaults = lib.mkOption {
      inherit (jsonFormat) type;
      default = { };
      description = ''
        Lower-priority Claude Code settings passed through the wrapper's
        read-only default layer. Runtime user settings are still managed by
        Home Manager's native {option}`programs.claude-code.settings` option.
      '';
    };

    dangerouslySkipPermissions = lib.mkOption {
      type = lib.types.bool;
      default = true;
      description = "Bake Claude Code's bypass-permissions flag into the wrapper.";
    };

    addDirs = lib.mkOption {
      type = lib.types.listOf pathLike;
      default = [ ];
      description = "Directories baked as Claude Code {command}`--add-dir=<dir>` flags.";
    };

    pluginDirs = lib.mkOption {
      type = lib.types.listOf pathLike;
      default = [ ];
      description = "Directories baked as Claude Code {command}`--plugin-dir=<dir>` flags.";
    };

    primaryCheckouts = lib.mkOption {
      type = lib.types.listOf lib.types.str;
      default = [
        "/home/*/index"
        "/home/*/ix"
      ];
      description = "Shell globs protected by the shared worktree guard hook.";
    };

    personalStartupContext = lib.mkOption {
      type = lib.types.bool;
      default = false;
      description = "Enable Andrew-only startup context hooks in the rendered Claude Code policy.";
    };

    defaultMcpServers = lib.mkOption {
      type = lib.types.nullOr jsonFormat.type;
      default = null;
      description = ''
        MCP server JSON to bake into the wrapper's default MCP layer. Null keeps
        the package default; `{ }` intentionally bakes no default MCP config.
        Home Manager's native {option}`programs.claude-code.mcpServers` remains
        the user config layer.
      '';
    };

    systemPrompt = lib.mkOption {
      type = lib.types.submodule {
        options = {
          source = lib.mkOption {
            type = systemPromptSource;
            default = "house";
            description = ''
              Which system prompt the wrapper bakes: `house` renders the
              structured house prompt, `stock` bakes no prompt flag, and `text`
              uses {option}`programs.claude-code.systemPrompt.text`.
            '';
          };

          text = lib.mkOption {
            type = lib.types.nullOr lib.types.lines;
            default = null;
            description = ''
              Replacement Claude Code system prompt when
              {option}`programs.claude-code.systemPrompt.source` is `text`.
            '';
          };

          omitRules = lib.mkOption {
            type = lib.types.listOf lib.types.str;
            default = [ ];
            description = ''
              Rule names omitted from the generated house system prompt. Only
              valid when {option}`programs.claude-code.systemPrompt.source` is
              `house`.
            '';
          };
        };
      };
      default = { };
      description = ''
        Structured control for the system prompt baked into the Claude Code
        wrapper.
      '';
    };
  };

  config = {
    assertions = [
      {
        assertion = (cfg.systemPrompt.source == "text") == (cfg.systemPrompt.text != null);
        message = "programs.claude-code.systemPrompt: source = \"text\" requires text, and text requires source = \"text\".";
      }
      {
        assertion = cfg.systemPrompt.source == "house" || cfg.systemPrompt.omitRules == [ ];
        message = "programs.claude-code.systemPrompt.omitRules only applies when source = \"house\".";
      }
    ];

    programs.claude-code.package = lib.mkDefault defaultedPackage;
  };
}
