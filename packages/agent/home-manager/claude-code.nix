{
  indexPackages,
  # Path to the house prompt module (packages/agent/prompt), injected by the
  # importing flake so this module never climbs the tree with `../`.
  promptModule,
  # The mutable-json home module (lib/services/mutable-json.nix), injected by
  # the importing flake; carries the last-applied 3-way merge that
  # materializes the wrapper's settings render into the writable user
  # settings.json (#3180). Keyed, so a config importing
  # `homeModules.mutable-json` alongside this module still declares the
  # option once.
  mutableJsonModule,
}: {
  config,
  lib,
  options,
  pkgs,
  ...
}: let
  cfg = config.programs.claude-code;
  jsonFormat = pkgs.formats.json {};
  pathLike = lib.types.either lib.types.path lib.types.str;
  indexPkgs = indexPackages pkgs.stdenv.hostPlatform.system;
  systemPromptSource = lib.types.enum [
    "house"
    "stock"
    "text"
  ];

  housePrompt = import promptModule {
    inherit lib;
    omitRules = cfg.houseContext.omitRules;
  };
  houseContextText = lib.concatStringsSep "\n\n" (
    [(housePrompt.contextFor "claude")]
    ++ lib.optional (cfg.houseContext.extraText != "") cfg.houseContext.extraText
  );

  optionalOverride = condition: name: value:
    lib.optionalAttrs condition {${name} = value;};
  packageOverrides =
    {
      inherit
        (cfg)
        addDirs
        dangerouslySkipPermissions
        features
        personalStartupContext
        primaryCheckouts
        systemTools
        ;
      # The index plugin (skills as `/index:<skill>`) rides the wrapper's
      # `--plugin-dir` layer ahead of any user-specified plugin dirs.
      pluginDirs =
        lib.optional cfg.housePlugin.enable indexPkgs.agent-plugin
        ++ cfg.pluginDirs;
      omitRules = cfg.systemPrompt.omitRules;
      extraSettings = cfg.defaults;
    }
    // optionalOverride (cfg.defaultMcpServers != null) "mcpServers" cfg.defaultMcpServers
    // optionalOverride (cfg.systemPrompt.source == "text") "systemPrompt" cfg.systemPrompt.text
    // optionalOverride (cfg.systemPrompt.source == "stock") "systemPrompt" null;
  defaultedPackage = cfg.basePackage.override packageOverrides;
in {
  imports = [mutableJsonModule];

  options.programs.claude-code = {
    basePackage = lib.mkOption {
      type = lib.types.package;
      default = indexPkgs.claude-code;
      defaultText = lib.literalExpression "inputs.index.packages.\${pkgs.stdenv.hostPlatform.system}.claude-code";
      description = "Base index Claude Code wrapper package before Home Manager applies defaults.";
    };

    defaults = lib.mkOption {
      inherit (jsonFormat) type;
      default = {};
      description = ''
        Claude Code settings folded into the wrapper's computed render
        (between the house posture defaults and the controlled keys the
        package owns). With {option}`programs.claude-code.materializeSettings`
        the merged render lands in the writable
        {file}`~/.claude/settings.json`, so these stay user-overridable at
        runtime and the live config is explainable from disk.
      '';
    };

    materializeSettings = lib.mkOption {
      type = lib.types.bool;
      default = true;
      description = ''
        Materialize the wrapper package's computed settings render
        (`passthru.settings`: house posture defaults, {option}`defaults`,
        then the controlled hooks/permissions/env keys) into the writable
        {file}`settings.json` under {option}`programs.claude-code.configDir`.
        Reconciled on activation with a last-applied 3-way merge
        (`homeModules.mutable-json`): declared keys are enforced, keys the
        render stops declaring are pruned, and Claude Code's own runtime
        writes (`/config` toggles, plugin state) survive. Requires
        {option}`programs.claude-code.package` to be the index wrapper (or
        any package exposing `passthru.settings`).
      '';
    };

    dangerouslySkipPermissions = lib.mkOption {
      type = lib.types.bool;
      default = true;
      description = "Bake Claude Code's bypass-permissions flag into the wrapper.";
    };

    features = lib.mkOption {
      type = lib.types.attrsOf (lib.types.nullOr (lib.types.either lib.types.bool lib.types.int));
      default = {};
      example = {
        context1M = true;
        autoCompactWindow = null;
      };
      description = ''
        Typed Claude Code feature posture forwarded to the wrapper's
        `features` argument: booleans gate features (false bakes the
        feature's CLAUDE_CODE_DISABLE_* env var into both the launch layer
        and the settings env), `autoCompactWindow` is a token count for
        CLAUDE_CODE_AUTO_COMPACT_WINDOW (null bakes nothing). Keys must
        exist in the wrapper's defaultFeatures table.
      '';
    };

    systemTools = lib.mkOption {
      type = lib.types.attrsOf lib.types.bool;
      default = {};
      example = {
        AskUserQuestion = true;
        DesignSync = true;
      };
      description = ''
        Overrides for Claude Code built-in orchestration and hosted-service
        tools. Tool names must be present in the wrapper's defaultSystemTools
        table. True enables the tool; false denies it.
      '';
    };

    addDirs = lib.mkOption {
      type = lib.types.listOf pathLike;
      default = [];
      description = "Directories baked as Claude Code {command}`--add-dir=<dir>` flags.";
    };

    pluginDirs = lib.mkOption {
      type = lib.types.listOf pathLike;
      default = [];
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

    housePlugin = {
      enable = lib.mkOption {
        type = lib.types.bool;
        default = true;
        description = ''
          Bake the index plugin (the repo skill set, invoked as
          `/index:<skill>`) into the wrapper as a {command}`--plugin-dir`
          layer. Disable to run without the house skills.
        '';
      };
    };

    houseContext = {
      enable = lib.mkOption {
        type = lib.types.bool;
        default = false;
        description = ''
          Write the house context render (the tagged prompt rules minus the
          `system`-only basics, see packages/agent/prompt) to
          {file}`~/.claude/CLAUDE.md` through the native
          {option}`programs.claude-code.context` option, so sessions whose
          runtime keeps its stock system prompt (claude.ai desktop, unwrapped
          CLIs) still ride the house rules. Keep this off when the consuming
          Home Manager configuration already manages {file}`.claude/CLAUDE.md`
          through {option}`home.file`.
        '';
      };

      extraText = lib.mkOption {
        type = lib.types.lines;
        default = "";
        description = ''
          Personal instructions appended after the house rules in the
          rendered context file.
        '';
      };

      omitRules = lib.mkOption {
        type = lib.types.listOf lib.types.str;
        default = [];
        description = ''
          Rule names omitted from the house context render (independent of
          {option}`programs.claude-code.systemPrompt.omitRules`, which governs
          the baked system prompt).
        '';
      };
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
            default = [];
            description = ''
              Rule names omitted from the generated house system prompt. Only
              valid when {option}`programs.claude-code.systemPrompt.source` is
              `house`.
            '';
          };
        };
      };
      default = {};
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
        assertion = cfg.systemPrompt.source == "house" || cfg.systemPrompt.omitRules == [];
        message = "programs.claude-code.systemPrompt.omitRules only applies when source = \"house\".";
      }
      {
        # omitRules reaches the shipped wrapper only through defaultedPackage
        # (basePackage.override packageOverrides); an explicit `package =`
        # discards that override. Left unchecked this shipped a half-applied
        # policy: the explicit package's permissions allowed force-merging
        # while its baked prompt still forbade it (index#3537). The package
        # stays defaulted only while no definition beats the module's own
        # `lib.mkDefault defaultedPackage` (numerically lower highestPrio
        # wins), so compare against that same mkDefault priority.
        assertion =
          cfg.systemPrompt.omitRules
          == []
          || options.programs.claude-code.package.highestPrio >= (lib.mkDefault null).priority;
        message = "programs.claude-code.systemPrompt.omitRules is ignored when package is set explicitly; pass omitRules to that package's override instead (index#3537).";
      }
      {
        # The upstream module renders settings.json as a read-only store
        # symlink whenever these options are set (settings, marketplaces, or
        # any disabled MCP server); the materialized file needs a single
        # declarative owner (see lib/services/mutable-json.nix).
        assertion =
          !(cfg.enable && cfg.materializeSettings)
          || (
            cfg.settings
            == {}
            && cfg.marketplaces == {}
            && lib.all (server: (server.enabled or null) != false && (server.disabled or false) != true) (
              lib.attrValues cfg.mcpServers
            )
          );
        message = "programs.claude-code.materializeSettings owns settings.json; move settings/marketplaces/disabled MCP servers into programs.claude-code.defaults (or disable materializeSettings).";
      }
    ];

    programs.claude-code = {
      package = lib.mkDefault defaultedPackage;
      context = lib.mkIf cfg.houseContext.enable (lib.mkDefault houseContextText);
    };

    # The wrapper injects no `--settings` flag (#3180): its computed render is
    # seeded into the writable user settings.json instead, where Claude Code's
    # own runtime writes survive the merge and every key stays overridable by
    # a project/local scope or a runtime toggle.
    home.mutableJsonFiles.claude-code-settings = lib.mkIf (cfg.enable && cfg.materializeSettings) {
      target = "${cfg.configDir}/settings.json";
      value = cfg.package.passthru.settings;
    };
  };
}
