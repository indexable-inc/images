# Content-validated subagent investigation cache daemon (ENG-4665).
#
# A small axum + Postgres daemon that serves a repeated read-only subagent
# investigation from a prior finding while every file that finding read is still
# byte-for-byte unchanged. Recall is Postgres full-text search; the only
# outbound call is a one-shot Haiku judge that gates precision. The Claude Code
# lookup/populate hooks (packages/agent/claude-hooks) own freshness re-hashing
# and capture, and POST to this daemon over the trusted tailnet.
#
# This module is deliberately generic: it owns the systemd service and the
# daemon's non-secret configuration, and takes the Postgres URL and the Anthropic
# key as runtime file paths. The consumer (e.g. the ix fleet) supplies those
# files through its own secret mechanism and adds placement, firewalling, and
# database-unit ordering by merging onto `systemd.services.subagent-cache`.
{
  config,
  lib,
  pkgs,
  ...
}: let
  inherit
    (lib)
    mkEnableOption
    mkIf
    mkOption
    mkPackageOption
    types
    ;
  cfg = config.services.subagent-cache;
in {
  options.services.subagent-cache = {
    enable = mkEnableOption "the content-validated subagent investigation cache daemon (ENG-4665)";

    package = mkPackageOption pkgs "subagent-cache" {};

    bind = mkOption {
      type = types.str;
      default = "127.0.0.1:8787";
      example = "100.64.0.1:3013";
      description = ''
        `addr:port` the HTTP API binds on. The Claude Code lookup and populate
        hooks POST to it; bind a tailnet address to keep the daemon reachable
        only over the trusted tailnet (it carries no TLS).
      '';
    };

    environmentFiles = mkOption {
      # Runtime path strings, not `types.path`: these are provided at boot by a
      # secret mechanism outside the image, not build inputs.
      type = types.listOf types.str;
      default = [];
      example = ["/run/subagent-cache/db.env"];
      description = ''
        systemd `EnvironmentFile` paths layered onto the daemon, for the two
        secrets it reads from the environment: `DATABASE_URL` (the Postgres
        connection string, password and all) and `ANTHROPIC_API_KEY` (the
        Stage-2 Haiku judge key). Kept out of the unit's `environment` (which
        lands in the world-readable store) and off argv. The consumer delivers
        these through its own secret mechanism (e.g. the ix fleet composes
        `DATABASE_URL` from the node's credential and injects `ANTHROPIC_API_KEY`
        via its secret store). The daemon fails fast at startup if either is
        missing, and bootstraps its own schema idempotently.
      '';
    };

    ttlDays = mkOption {
      type = types.ints.positive;
      default = 7;
      description = ''
        TTL backstop in days: an entry expires this long after its last populate
        even if none of its files ever change.
      '';
    };

    recallTopK = mkOption {
      type = types.ints.positive;
      default = 3;
      description = "Number of full-text recall candidates the judge may inspect per lookup.";
    };

    recallFloor = mkOption {
      type = types.float;
      default = 0.01;
      description = ''
        Minimum `ts_rank` a candidate must score to reach the judge. Below it the
        lookup is a cheap miss and the judge never fires.
      '';
    };

    judgeApiBase = mkOption {
      type = types.str;
      default = "https://api.anthropic.com";
      description = "Anthropic Messages API base URL for the Stage-2 judge.";
    };

    judgeModel = mkOption {
      type = types.str;
      default = "claude-haiku-4-5";
      description = "Judge model id (a Haiku-class model).";
    };
  };

  config = mkIf cfg.enable {
    systemd.services.subagent-cache = {
      description = "Content-validated subagent investigation cache (ENG-4665)";
      wantedBy = ["multi-user.target"];
      after = ["network-online.target"];
      wants = ["network-online.target"];
      environment = {
        SUBAGENT_CACHE_BIND = cfg.bind;
        SUBAGENT_CACHE_TTL_DAYS = toString cfg.ttlDays;
        SUBAGENT_CACHE_TOP_K = toString cfg.recallTopK;
        SUBAGENT_CACHE_RECALL_FLOOR = toString cfg.recallFloor;
        SUBAGENT_CACHE_JUDGE_API_BASE = cfg.judgeApiBase;
        SUBAGENT_CACHE_JUDGE_MODEL = cfg.judgeModel;
        # A hardened unit's ProtectSystem hides the host trust store, so the
        # judge's HTTPS call to Anthropic needs an explicit CA bundle.
        SSL_CERT_FILE = "${pkgs.cacert}/etc/ssl/certs/ca-bundle.crt";
      };
      serviceConfig = {
        # DATABASE_URL and ANTHROPIC_API_KEY arrive here, kept out of argv and the
        # store. The daemon reads both from the environment and fails fast if
        # either is absent.
        EnvironmentFile = cfg.environmentFiles;
        ExecStart = lib.getExe cfg.package;
        Restart = "on-failure";
        RestartSec = 5;
      };
    };
  };
}
