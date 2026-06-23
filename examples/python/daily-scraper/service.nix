{
  config,
  ix,
  lib,
  pkgs,
  ...
}:
let
  dailyScraper = config._module.args.dailyScraper or { };
  dataDir = dailyScraper.dataDir or "/var/lib/daily-scraper";
  s3Settings = dailyScraper.s3 or { };
  scraper = {
    package = dailyScraper.package or (import ./package.nix { inherit ix lib pkgs; });
    repository = dailyScraper.repository or "indexable-inc/index";
    githubApiUrl = dailyScraper.githubApiUrl or "https://api.github.com";
    userAgent = dailyScraper.userAgent or "ix-daily-scraper-example/0.1";
    schedule = dailyScraper.schedule or "*-*-* 03:17:00 UTC";
    randomizedDelaySec = dailyScraper.randomizedDelaySec or "20m";
    extraArgs = dailyScraper.extraArgs or [ ];
    environment = dailyScraper.environment or { };
    inherit dataDir;
    outputDir = dailyScraper.outputDir or "${dataDir}/parquet";
    s3 = {
      uri = s3Settings.uri or null;
      deleteRemoved = s3Settings.deleteRemoved or false;
      awsEnvironmentFile = s3Settings.awsEnvironmentFile or null;
    };
  };

  scraperArgs = [
    (lib.getExe scraper.package)
    "--output-dir"
    scraper.outputDir
    "--repo"
    scraper.repository
    "--github-api-url"
    scraper.githubApiUrl
    "--user-agent"
    scraper.userAgent
  ]
  ++ scraper.extraArgs;

  environment = [
    "PYTHONUNBUFFERED=1"
  ]
  ++ lib.mapAttrsToList (name: value: "${name}=${value}") scraper.environment;

  s3Args = [
    (lib.getExe pkgs.awscli2)
    "s3"
    "sync"
    "--only-show-errors"
    scraper.outputDir
    scraper.s3.uri
  ]
  ++ lib.optional scraper.s3.deleteRemoved "--delete";
in
{
  environment.systemPackages = [ scraper.package ];

  ix.healthChecks.daily-scraper = {
    description = "Daily scraper timer is active";
    unit = "daily-scraper.timer";
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
        WorkingDirectory = scraper.dataDir;
        ExecStart = lib.escapeShellArgs scraperArgs;
        Environment = environment;
      }
      // lib.optionalAttrs (scraper.s3.uri != null) {
        ExecStartPost = lib.escapeShellArgs s3Args;
      }
      // lib.optionalAttrs (scraper.s3.awsEnvironmentFile != null) {
        LoadCredential = [ "aws-env:${scraper.s3.awsEnvironmentFile}" ];
        EnvironmentFile = "%d/aws-env";
      };
  };

  systemd.timers.daily-scraper = {
    description = "Run the daily Python scraper";
    wantedBy = [ "timers.target" ];
    timerConfig = {
      OnCalendar = scraper.schedule;
      Persistent = true;
      RandomizedDelaySec = scraper.randomizedDelaySec;
      AccuracySec = "5m";
      Unit = "daily-scraper.service";
    };
  };
}
