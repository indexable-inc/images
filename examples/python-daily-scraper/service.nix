{
  config,
  ix,
  lib,
  pkgs,
  ...
}:
let
  inherit (lib)
    mkEnableOption
    mkIf
    mkOption
    types
    ;

  cfg = config.services.daily-scraper;
  package = import ./package.nix { inherit ix lib pkgs; };
  dataDir = "/var/lib/daily-scraper";
  outputDir = "${dataDir}/parquet";
  systemctl = lib.getExe' config.systemd.package "systemctl";

  scraperArgs = [
    (lib.getExe cfg.package)
    "--output-dir"
    outputDir
    "--repo"
    cfg.repository
    "--github-api-url"
    cfg.githubApiUrl
    "--user-agent"
    cfg.userAgent
  ]
  ++ cfg.extraArgs;

  environment = [
    "PYTHONUNBUFFERED=1"
  ]
  ++ lib.mapAttrsToList (name: value: "${name}=${value}") cfg.environment;

  s3Args = [
    (lib.getExe pkgs.awscli2)
    "s3"
    "sync"
    "--only-show-errors"
    outputDir
    cfg.s3.uri
  ]
  ++ lib.optionals cfg.s3.deleteRemoved [ "--delete" ];
in
{
  options.services.daily-scraper = {
    enable = mkEnableOption "daily Python scraper";

    package = mkOption {
      type = types.package;
      default = package;
      description = "Python scraper package to execute.";
    };

    repository = mkOption {
      type = types.str;
      default = "indexable-inc/index";
      description = "GitHub repository fetched by the example scraper.";
    };

    githubApiUrl = mkOption {
      type = types.str;
      default = "https://api.github.com";
      description = "GitHub API base URL.";
    };

    userAgent = mkOption {
      type = types.str;
      default = "ix-daily-scraper-example/0.1";
      description = "HTTP User-Agent sent by the scraper.";
    };

    schedule = mkOption {
      type = types.str;
      default = "*-*-* 03:17:00 UTC";
      description = "systemd calendar expression for the daily run.";
    };

    randomizedDelaySec = mkOption {
      type = types.str;
      default = "20m";
      description = "Maximum timer jitter added by systemd.";
    };

    extraArgs = mkOption {
      type = types.listOf types.str;
      default = [ ];
      description = "Additional command-line arguments passed to the scraper.";
    };

    environment = mkOption {
      type = types.attrsOf types.str;
      default = { };
      description = "Extra environment variables for the scraper process.";
    };

    s3 = {
      uri = mkOption {
        type = types.nullOr types.str;
        default = null;
        example = "s3://andrew-scraper-output/github";
        description = "Optional S3 URI synced after a successful run.";
      };

      deleteRemoved = mkOption {
        type = types.bool;
        default = false;
        description = "Pass --delete to aws s3 sync.";
      };

      awsEnvironmentFile = mkOption {
        type = types.nullOr types.str;
        default = null;
        example = "/run/secrets/daily-scraper/aws.env";
        description = "Runtime path to an AWS EnvironmentFile loaded through systemd credentials.";
      };
    };
  };

  config = mkIf cfg.enable {
    environment.systemPackages = [ cfg.package ];

    ix.healthChecks.daily-scraper = {
      from = "guest";
      description = "Daily scraper timer is active";
      command = [
        systemctl
        "is-active"
        "--quiet"
        "daily-scraper.timer"
      ];
    };

    systemd.services.daily-scraper = {
      description = "Daily Python data scraper";
      after = [ "network-online.target" ];
      wants = [ "network-online.target" ];
      serviceConfig =
        ix.systemdHardening
        // {
          Type = "oneshot";
          DynamicUser = true;
          StateDirectory = "daily-scraper";
          StateDirectoryMode = "0750";
          WorkingDirectory = dataDir;
          ExecStart = lib.escapeShellArgs scraperArgs;
          Environment = environment;
        }
        // lib.optionalAttrs (cfg.s3.uri != null) {
          ExecStartPost = lib.escapeShellArgs s3Args;
        }
        // lib.optionalAttrs (cfg.s3.awsEnvironmentFile != null) {
          LoadCredential = [ "aws-env:${cfg.s3.awsEnvironmentFile}" ];
          EnvironmentFile = "%d/aws-env";
        };
    };

    systemd.timers.daily-scraper = {
      description = "Run the daily Python scraper";
      wantedBy = [ "timers.target" ];
      timerConfig = {
        OnCalendar = cfg.schedule;
        Persistent = true;
        RandomizedDelaySec = cfg.randomizedDelaySec;
        AccuracySec = "5m";
        Unit = "daily-scraper.service";
      };
    };
  };
}
