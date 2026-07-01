{ indexPackages }:
{
  config,
  lib,
  pkgs,
  ...
}:
let
  cfg = config.programs.codex;
  tomlFormat = pkgs.formats.toml { };
  jsonFormat = pkgs.formats.json { };
  pathLike = lib.types.either lib.types.path lib.types.str;
  indexPkgs = indexPackages pkgs.stdenv.hostPlatform.system;

  optionalOverride =
    condition: name: value:
    lib.optionalAttrs condition { ${name} = value; };
  packageOverrides = {
    inherit (cfg)
      forcedSettings
      personalStartupContext
      primaryCheckouts
      ;
    settings = cfg.defaults;
  }
  // optionalOverride (cfg.mcpServers != null) "mcpServers" cfg.mcpServers
  // optionalOverride (
    cfg.modelInstructionsFile != null
  ) "modelInstructionsFile" cfg.modelInstructionsFile
  // optionalOverride (cfg.systemPrompt != null) "systemPrompt" cfg.systemPrompt
  // optionalOverride cfg.disableModelInstructions "systemPrompt" null;
  finalPackage = cfg.basePackage.override packageOverrides;
in
{
  options.programs.codex = {
    basePackage = lib.mkOption {
      type = lib.types.package;
      default = indexPkgs.codex;
      defaultText = lib.literalExpression "inputs.index.packages.\${pkgs.stdenv.hostPlatform.system}.codex";
      description = "Base index Codex wrapper package before Home Manager applies defaults.";
    };

    finalPackage = lib.mkOption {
      type = lib.types.package;
      readOnly = true;
      description = "Codex package after Home Manager defaults are applied.";
    };

    defaults = lib.mkOption {
      inherit (tomlFormat) type;
      default = {
        features.multi_agent_v2 = {
          enabled = true;
          max_concurrent_threads_per_session = 16;
        };
        agents.max_depth = 3;
      };
      description = ''
        Lower-priority Codex config rendered through the wrapper's default
        layer. These values are used only when the user's
        {file}`config.toml` does not already set the same key.
      '';
    };

    forcedSettings = lib.mkOption {
      inherit (tomlFormat) type;
      default = {
        check_for_update_on_startup = false;
      };
      description = ''
        Codex settings rendered as highest-precedence forced {command}`-c`
        wrapper flags. Use this for invariants, not user-overridable defaults.
      '';
    };

    primaryCheckouts = lib.mkOption {
      type = lib.types.listOf lib.types.str;
      default = [
        "/home/*/index"
        "/home/*/ix"
      ];
      description = "Shell globs threaded into the shared agent hook policy.";
    };

    personalStartupContext = lib.mkOption {
      type = lib.types.bool;
      default = false;
      description = "Enable Andrew-only startup context hooks in the rendered Codex policy.";
    };

    mcpServers = lib.mkOption {
      type = lib.types.nullOr jsonFormat.type;
      default = null;
      description = ''
        MCP server declarations rendered as soft Codex config defaults. Null
        keeps the package default; `{ }` intentionally bakes no MCP defaults.
      '';
    };

    modelInstructionsFile = lib.mkOption {
      type = lib.types.nullOr pathLike;
      default = null;
      description = ''
        Existing file to use for Codex's {option}`model_instructions_file`.
        Null keeps the package default generated from the house prompt.
      '';
    };

    systemPrompt = lib.mkOption {
      type = lib.types.nullOr lib.types.lines;
      default = null;
      description = ''
        Replacement Codex model instructions text. Null keeps the package
        default house prompt.
      '';
    };

    disableModelInstructions = lib.mkOption {
      type = lib.types.bool;
      default = false;
      description = "Do not bake a {option}`model_instructions_file` default into the Codex wrapper.";
    };

    installHooks = lib.mkOption {
      type = lib.types.bool;
      default = true;
      description = "Install the shared Codex hook policy into the configured Codex home.";
    };

    configDir = lib.mkOption {
      type = lib.types.str;
      default = ".codex";
      description = "Codex config directory, relative to the Home Manager home directory.";
    };
  };

  config = {
    assertions = [
      {
        assertion =
          lib.count (x: x) [
            cfg.disableModelInstructions
            (cfg.modelInstructionsFile != null)
            (cfg.systemPrompt != null)
          ] <= 1;
        message = "programs.codex: choose only one of disableModelInstructions, modelInstructionsFile, or systemPrompt.";
      }
    ];

    programs.codex = {
      package = lib.mkDefault finalPackage;
      inherit finalPackage;
    };

    home.file."${cfg.configDir}/hooks.json" = lib.mkIf (cfg.enable && cfg.installHooks) {
      source = cfg.finalPackage.hooksJson;
    };
  };
}
