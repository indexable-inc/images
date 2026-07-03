# Single-node S3-compatible object storage via SeaweedFS.
#
# One `weed server -s3` process runs the master, volume, filer, and S3
# gateway in a single binary: enabling `-s3` auto-starts the filer it
# depends on, so a single node needs no orchestration. SeaweedFS is the
# fastest single-node S3 surface in nixpkgs (Apache-2.0, best small-object
# IOPS, top large-object throughput); MinIO is AGPL with a gutted OSS
# console, Garage is AGPL and weaker on small objects.
#
# Listeners bind broadly (`-ip.bind`) but only the S3 port is opened in the
# firewall, so master/volume/filer stay node-local while S3 is reachable.
{
  config,
  ix,
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
    optional
    types
    ;
  cfg = config.services.ix-seaweedfs;
  weed = lib.getExe' cfg.package "weed";

  # `weed server` refuses to start unless `-dir` already exists and is
  # writable: it does not create it. systemd's `StateDirectory` creates and
  # chowns this exact path to the dynamic user before ExecStart, so the two
  # must stay equal. Tying `-dir` to the StateDirectory keeps that invariant
  # instead of exposing a free-form path that systemd would not provision.
  stateDir = "/var/lib/seaweedfs";

  # `-ip` is the address peers advertise to each other; on one node the
  # internal services only ever talk over loopback. `-ip.bind` is the
  # listen address, kept wide so the S3 port is reachable off-host while
  # the firewall (below) is what actually limits exposure to S3 alone.
  args =
    [
      "server"
      "-dir=${stateDir}"
      "-ip=127.0.0.1"
      "-ip.bind=${cfg.bindAddress}"
      "-s3"
      "-s3.port=${toString cfg.port}"
    ]
    ++ optional (cfg.configFile != null) "-s3.config=${cfg.configFile}"
    ++ cfg.extraArgs;
in {
  options.services.ix-seaweedfs = {
    enable = mkEnableOption "SeaweedFS single-node S3 object storage";

    package = mkPackageOption pkgs "seaweedfs" {};

    port = mkOption {
      type = types.port;
      default = 8333;
      description = "S3 gateway listen port.";
    };

    bindAddress = mkOption {
      type = types.str;
      default = "0.0.0.0";
      description = ''
        Listen address for every SeaweedFS listener. Exposure is bounded
        by the firewall, which only opens {option}`port`; the master,
        volume, and filer ports stay closed regardless of this value.
      '';
    };

    openFirewall = mkOption {
      type = types.bool;
      default = true;
      description = "Whether to open the S3 port in the firewall.";
    };

    configFile = mkOption {
      type = types.nullOr types.path;
      default = null;
      example = "/run/secrets/seaweedfs-s3.json";
      description = ''
        Path to the SeaweedFS S3 identities config (`-s3.config`) holding
        access/secret key pairs and per-identity actions. Point this at a
        runtime secret file rather than a store path so credentials never
        enter the Nix store. When unset, set {option}`allowAnonymous`.
      '';
    };

    allowAnonymous = mkEnableOption ''
      unauthenticated S3 access. Without {option}`configFile` SeaweedFS
      serves every bucket to anonymous callers; this option is the
      explicit opt-in required to run that way'';

    extraArgs = mkOption {
      type = types.listOf types.str;
      default = [];
      example = ["-volume.max=0"];
      description = "Extra arguments appended to `weed server`.";
    };
  };

  config = mkIf cfg.enable {
    # Refuse the dangerous silent default: an S3 endpoint with no
    # credentials is a data-exposure footgun, so require the operator to
    # supply identities or opt into anonymous access by name.
    assertions = [
      {
        assertion = cfg.configFile != null || cfg.allowAnonymous;
        message = ''
          services.ix-seaweedfs: set `configFile` to an S3 identities file
          or enable `allowAnonymous` to run without authentication.
        '';
      }
    ];

    ix.networking.portClaims.ix-seaweedfs = {
      protocol = "tcp";
      inherit (cfg) port;
      description = "SeaweedFS S3 gateway";
    };

    networking.firewall.allowedTCPPorts = optional cfg.openFirewall cfg.port;

    ix.healthChecks.ix-seaweedfs = {
      from = "guest";
      # `/healthz` is served unauthenticated by the S3 gateway and only
      # responds once the gateway (and the filer it proxies) are up, so it
      # is a real readiness probe rather than a unit-active check.
      description = "SeaweedFS S3 gateway is serving";
      command = [
        (lib.getExe pkgs.curl)
        "--fail"
        "--silent"
        "--show-error"
        "--max-time"
        "5"
        "http://127.0.0.1:${toString cfg.port}/healthz"
      ];
    };

    systemd.services.ix-seaweedfs = {
      description = "SeaweedFS single-node S3 object storage";
      after = ["network-online.target"];
      wants = ["network-online.target"];
      wantedBy = ["multi-user.target"];
      serviceConfig =
        ix.systemdHardening
        // {
          Type = "simple";
          ExecStart = "${weed} ${lib.escapeShellArgs args}";
          Restart = "on-failure";
          RestartSec = 2;
          DynamicUser = true;
          StateDirectory = "seaweedfs";
          # `ProtectSystem=strict` makes CWD (`/`) read-only. `weed` resolves
          # optional config files (filer.toml, security.toml) relative to CWD,
          # so point the working directory at the writable state dir.
          WorkingDirectory = stateDir;
        };
    };
  };
}
