# home-manager module exposing `services.indexer`: run the `indexer` on a timer
# as the current user, syncing the selected corpus sources (agent/shell history,
# Slack/Linear exports, git repos) into an S3/R2 parquet archive and/or a
# Mixedbread store.
#
# Closed over the per-system flake package set so it resolves the `indexer`
# derivation for the host it runs on, and over the portable-services home module
# so one spec renders a native launchd agent on macOS and a native systemd user
# unit (plus timer) on Linux. See `lib/portable-services.nix` and
# `users/andrewgazelka/home.nix`.
#
# Auth, by design, never lands secrets in the Nix store:
#   * Mixedbread: `MXBAI_API_KEY` in `environment` if set, otherwise the
#     `mgrep login` token at `~/.mgrep/token.json` (the agent runs as the user).
#   * S3/R2: `AWS_ACCESS_KEY_ID` / `AWS_SECRET_ACCESS_KEY`. `environment` is
#     rendered into the agent/unit in the world-readable Nix store, so pass real
#     credentials through a wrapper that sources a runtime file rather than
#     inlining them here.
{
  indexPackages,
  portableServicesModule,
}:
{
  config,
  lib,
  pkgs,
  ...
}:
let
  inherit (lib)
    mkOption
    mkEnableOption
    mkIf
    types
    optional
    optionals
    concatMap
    ;

  cfg = config.services.indexer;

  defaultPackage = (indexPackages pkgs.stdenv.hostPlatform.system).indexer;

  sourceFlags =
    optional cfg.local "--local"
    ++ optionals (cfg.claudeDir != null) [
      "--claude-dir"
      cfg.claudeDir
    ]
    ++ optionals (cfg.codexFile != null) [
      "--codex-file"
      cfg.codexFile
    ]
    ++ optionals (cfg.atuinDb != null) [
      "--atuin-db"
      cfg.atuinDb
    ]
    ++ optionals (cfg.slackExport != null) [
      "--slack-export"
      cfg.slackExport
    ]
    ++ optionals (cfg.linearExport != null) [
      "--linear-export"
      cfg.linearExport
    ]
    ++ concatMap (repo: [
      "--git-repo"
      repo
    ]) cfg.gitRepos;

  sinkFlags =
    optionals (cfg.bucket != null) (
      [
        "--bucket"
        cfg.bucket
        "--region"
        cfg.region
        "--prefix"
        cfg.prefix
      ]
      ++ optionals (cfg.endpoint != null) [
        "--endpoint"
        cfg.endpoint
      ]
    )
    ++ optionals (cfg.mixedbreadStore != null) (
      [
        "--mixedbread-store"
        cfg.mixedbreadStore
      ]
      ++ optionals (cfg.baseUrl != null) [
        "--base-url"
        cfg.baseUrl
      ]
    );
in
{
  imports = [ portableServicesModule ];

  options.services.indexer = {
    enable = mkEnableOption "syncing the selected corpus sources to an S3/R2 parquet archive and/or a Mixedbread store";

    package = mkOption {
      type = types.package;
      default = defaultPackage;
      defaultText = lib.literalExpression "index.packages.\${system}.indexer";
      description = "The `indexer` package to run.";
    };

    local = mkOption {
      type = types.bool;
      default = false;
      description = "Index local agent/shell history (claude, codex, atuin) at their default paths.";
    };

    claudeDir = mkOption {
      type = types.nullOr types.str;
      default = null;
      description = "Claude Code transcript directory override.";
    };

    codexFile = mkOption {
      type = types.nullOr types.str;
      default = null;
      description = "Codex history file override.";
    };

    atuinDb = mkOption {
      type = types.nullOr types.str;
      default = null;
      description = "atuin history db override.";
    };

    slackExport = mkOption {
      type = types.nullOr types.str;
      default = null;
      description = "Slack export directory to ingest.";
    };

    linearExport = mkOption {
      type = types.nullOr types.str;
      default = null;
      description = "Linear export directory to ingest.";
    };

    gitRepos = mkOption {
      type = types.listOf types.str;
      default = [ ];
      description = "Git repositories whose commit history to ingest.";
    };

    bucket = mkOption {
      type = types.nullOr types.str;
      default = null;
      description = "Enable the S3/R2 parquet sink to this bucket. Needs AWS credentials in the process environment.";
    };

    endpoint = mkOption {
      type = types.nullOr types.str;
      default = null;
      description = "S3 endpoint URL (an R2 account endpoint or a MinIO URL). Null targets AWS S3.";
    };

    region = mkOption {
      type = types.str;
      default = "auto";
      description = "S3 region label (`auto` for R2).";
    };

    prefix = mkOption {
      type = types.str;
      default = "corpus";
      description = "Key prefix under the bucket.";
    };

    mixedbreadStore = mkOption {
      type = types.nullOr types.str;
      default = null;
      description = "Enable the Mixedbread sink into this store. Auth via `MXBAI_API_KEY`, else the `mgrep login` token.";
    };

    baseUrl = mkOption {
      type = types.nullOr types.str;
      default = null;
      description = "Mixedbread API base URL override.";
    };

    interval = mkOption {
      type = types.ints.positive;
      default = 86400;
      description = "Run the sync every N seconds (default: daily).";
    };

    environment = mkOption {
      type = types.attrsOf types.str;
      default = { };
      example = {
        AWS_ACCESS_KEY_ID = "...";
      };
      description = ''
        Extra environment for the process (S3 credentials, `MXBAI_API_KEY`).

        Rendered into the launchd agent / systemd user unit in the
        world-readable Nix store, so do not inline secrets: reference a wrapper
        that sources a runtime credentials file, or rely on the `mgrep login`
        token for Mixedbread.
      '';
    };
  };

  config = mkIf cfg.enable {
    assertions = [
      {
        assertion = cfg.bucket != null || cfg.mixedbreadStore != null;
        message = "services.indexer: set bucket and/or mixedbreadStore (at least one sink).";
      }
      {
        assertion =
          cfg.local
          || cfg.claudeDir != null
          || cfg.codexFile != null
          || cfg.atuinDb != null
          || cfg.slackExport != null
          || cfg.linearExport != null
          || cfg.gitRepos != [ ];
        message = "services.indexer: select at least one source (local, a *Dir/*File/*Export path, or gitRepos).";
      }
    ];

    services.portable.indexer = {
      description = "Sync corpus sources to parquet + Mixedbread";
      command = [ "${cfg.package}/bin/indexer" ] ++ sourceFlags ++ sinkFlags;
      inherit (cfg) environment interval;
    };
  };
}
