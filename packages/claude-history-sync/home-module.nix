# home-manager module exposing `services.claudeHistorySync`: run
# `claude-history-sync` on a timer as the current user, landing this user's
# Claude Code transcripts in an S3/R2 parquet archive and/or a Mixedbread store.
#
# Closed over the per-system flake package set so it resolves the
# `claude-history-sync` derivation for the host it runs on, and over the
# portable-services home module so one spec renders a native launchd agent on
# macOS and a native systemd user unit (plus timer) on Linux. See
# `lib/portable-services.nix` and `users/andrewgazelka/home.nix`.
#
# Auth, by design, never lands secrets in the Nix store:
#   * Mixedbread: `MXBAI_API_KEY` in `environment` if set, otherwise the
#     `mgrep login` token at `~/.mgrep/token.json` (the agent runs as the user,
#     so the token file is readable without any store-side secret).
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
    optionals
    ;

  cfg = config.services.claudeHistorySync;

  defaultPackage = (indexPackages pkgs.stdenv.hostPlatform.system).claude-history-sync;

  s3Flags = optionals (cfg.r2Bucket != null) (
    [
      "--r2-bucket"
      cfg.r2Bucket
      "--r2-region"
      cfg.r2Region
      "--prefix"
      cfg.prefix
    ]
    ++ optionals (cfg.r2Endpoint != null) [
      "--r2-endpoint"
      cfg.r2Endpoint
    ]
  );

  mixedbreadFlags = optionals (cfg.mixedbreadStore != null) (
    [
      "--mixedbread-store"
      cfg.mixedbreadStore
    ]
    ++ optionals (cfg.baseUrl != null) [
      "--base-url"
      cfg.baseUrl
    ]
  );

  dirFlags = optionals (cfg.dir != null) [
    "--dir"
    cfg.dir
  ];
in
{
  imports = [ portableServicesModule ];

  options.services.claudeHistorySync = {
    enable = mkEnableOption "syncing this user's Claude Code history to an S3/R2 parquet archive and/or a Mixedbread store";

    package = mkOption {
      type = types.package;
      default = defaultPackage;
      defaultText = lib.literalExpression "index.packages.\${system}.claude-history-sync";
      description = "The `claude-history-sync` package to run.";
    };

    dir = mkOption {
      type = types.nullOr types.str;
      default = null;
      description = "Transcript directory. Null uses the binary default (`~/.claude/projects`).";
    };

    r2Bucket = mkOption {
      type = types.nullOr types.str;
      default = null;
      description = "Enable the S3/R2 parquet sink to this bucket. Needs AWS credentials in the process environment.";
    };

    r2Endpoint = mkOption {
      type = types.nullOr types.str;
      default = null;
      description = "S3 endpoint URL (an R2 account endpoint or a MinIO URL). Null targets AWS S3.";
    };

    r2Region = mkOption {
      type = types.str;
      default = "auto";
      description = "S3 region label (`auto` for R2).";
    };

    prefix = mkOption {
      type = types.str;
      default = "claude-history";
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
        assertion = cfg.r2Bucket != null || cfg.mixedbreadStore != null;
        message = "services.claudeHistorySync: set r2Bucket and/or mixedbreadStore (at least one sink).";
      }
    ];

    services.portable.claude-history-sync = {
      description = "Sync Claude Code history to parquet + Mixedbread";
      command = [ "${cfg.package}/bin/claude-history-sync" ] ++ dirFlags ++ s3Flags ++ mixedbreadFlags;
      environment = cfg.environment;
      interval = cfg.interval;
    };
  };
}
