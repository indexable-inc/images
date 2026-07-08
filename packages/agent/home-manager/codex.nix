{
  indexPackages,
  # Path to the house prompt module (packages/agent/prompt), injected by the
  # importing flake so this module never climbs the tree with `../`.
  promptModule,
}: {
  config,
  lib,
  pkgs,
  ...
}: let
  cfg = config.programs.codex;
  tomlFormat = pkgs.formats.toml {};
  jsonFormat = pkgs.formats.json {};
  pathLike = lib.types.either lib.types.path lib.types.str;
  indexPkgs = indexPackages pkgs.stdenv.hostPlatform.system;
  systemPromptSource = lib.types.enum [
    "house"
    "stock"
    "text"
    "file"
  ];

  housePrompt = import promptModule {
    inherit lib;
    omitRules = cfg.houseContext.omitRules;
  };
  houseContextText = lib.concatStringsSep "\n\n" (
    [(housePrompt.contextFor "codex")]
    ++ lib.optional (cfg.houseContext.extraText != "") cfg.houseContext.extraText
  );

  # The index plugin reaches Codex as a config-declared local marketplace:
  # Codex resolves the Claude-format `.claude-plugin/{marketplace,plugin}.json`
  # manifests in place (no snapshot sync for local sources), gated behind the
  # `features.plugins` flag. Soft-layer entries, so any explicit user
  # config.toml value wins per key.
  housePluginSettings = lib.optionalAttrs cfg.housePlugin.enable {
    features.plugins = true;
    marketplaces.index = {
      source_type = "local";
      source = "${indexPkgs.agent-plugin.marketplace}";
    };
    plugins."index@index".enabled = true;
  };

  optionalOverride = condition: name: value:
    lib.optionalAttrs condition {${name} = value;};
  packageOverrides =
    {
      inherit
        (cfg)
        forcedSettings
        personalStartupContext
        primaryCheckouts
        ;
      # Manual two-level merge (same shape as the wrapper's forcedSettings
      # fold): user defaults win at the top level, and the `features` subtree
      # is combined so the plugin gate coexists with feature defaults.
      settings =
        housePluginSettings
        // cfg.defaults
        // {
          features = (housePluginSettings.features or {}) // (cfg.defaults.features or {});
        };
    }
    // optionalOverride (cfg.mcpServers != null) "mcpServers" cfg.mcpServers
    // optionalOverride (
      cfg.systemPrompt.source == "file"
    ) "modelInstructionsFile"
    cfg.systemPrompt.file
    // optionalOverride (cfg.systemPrompt.source == "text") "systemPrompt" cfg.systemPrompt.text
    // optionalOverride (cfg.systemPrompt.source == "stock") "systemPrompt" null
    // optionalOverride (cfg.systemPrompt.source == "house") "omitRules" cfg.systemPrompt.omitRules;
  finalPackage = cfg.basePackage.override packageOverrides;
in {
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

    housePlugin = {
      enable = lib.mkOption {
        type = lib.types.bool;
        default = true;
        description = ''
          Declare the index plugin (the repo skill set, invoked as
          `/index:<skill>`) through soft config defaults: a local marketplace
          entry pointing at the store-built Claude-format plugin bundle plus
          its enablement. Requires a Codex new enough to gate plugins behind
          `features.plugins`; older builds ignore the keys.
        '';
      };
    };

    houseContext = {
      enable = lib.mkOption {
        type = lib.types.bool;
        default = true;
        description = ''
          Write the house context render (the tagged prompt rules minus the
          `system`-only basics, see packages/agent/prompt) to
          {file}`$CODEX_HOME/AGENTS.md` through the native
          {option}`programs.codex.context` option. An explicit `context`
          value overrides this default entirely.
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
          {option}`programs.codex.systemPrompt.omitRules`, which governs the
          baked model instructions).
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
              Which model instructions the wrapper bakes: `house` renders the
              structured house prompt, `stock` bakes no default instructions,
              `text` materializes {option}`programs.codex.systemPrompt.text`,
              and `file` points at {option}`programs.codex.systemPrompt.file`.
            '';
          };

          text = lib.mkOption {
            type = lib.types.nullOr lib.types.lines;
            default = null;
            description = ''
              Replacement Codex model instructions when
              {option}`programs.codex.systemPrompt.source` is `text`.
            '';
          };

          file = lib.mkOption {
            type = lib.types.nullOr pathLike;
            default = null;
            description = ''
              Existing file to use for Codex's {option}`model_instructions_file`
              when {option}`programs.codex.systemPrompt.source` is `file`.
            '';
          };

          omitRules = lib.mkOption {
            type = lib.types.listOf lib.types.str;
            default = [];
            description = ''
              Rule names omitted from the generated house system prompt. Only
              valid when {option}`programs.codex.systemPrompt.source` is
              `house`.
            '';
          };
        };
      };
      default = {};
      description = ''
        Structured control for the system prompt rendered as Codex model
        instructions.
      '';
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
        assertion = (cfg.systemPrompt.source == "text") == (cfg.systemPrompt.text != null);
        message = "programs.codex.systemPrompt: source = \"text\" requires text, and text requires source = \"text\".";
      }
      {
        assertion = (cfg.systemPrompt.source == "file") == (cfg.systemPrompt.file != null);
        message = "programs.codex.systemPrompt: source = \"file\" requires file, and file requires source = \"file\".";
      }
      {
        assertion = cfg.systemPrompt.source == "house" || cfg.systemPrompt.omitRules == [];
        message = "programs.codex.systemPrompt.omitRules only applies when source = \"house\".";
      }
    ];

    programs.codex = {
      package = lib.mkDefault finalPackage;
      inherit finalPackage;
      context = lib.mkIf cfg.houseContext.enable (lib.mkDefault houseContextText);
    };

    home.file."${cfg.configDir}/hooks.json" = lib.mkIf (cfg.enable && cfg.installHooks) {
      source = cfg.finalPackage.hooksJson;
    };

    # Codex marks a plugin installed by the presence of
    # plugins/cache/<marketplace>/<plugin>/<version>; materialize that
    # snapshot from the store so the config-declared plugin is installed
    # without ever running `codex plugin add`. `recursive` because Codex
    # requires the version path to be a real directory (a bare symlink reads
    # as not installed; real dirs with per-file symlinks are accepted,
    # verified against codex plugin list).
    home.file."${cfg.configDir}/plugins/cache/index/${indexPkgs.agent-plugin.pluginName}/${indexPkgs.agent-plugin.version}" = lib.mkIf (cfg.enable && cfg.housePlugin.enable) {
      source = indexPkgs.agent-plugin;
      recursive = true;
    };
  };
}
