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

  cfg = config.services.resource-monitor;
  fs = lib.fileset;

  siteSrc = fs.toSource {
    root = ./site;
    fileset = fs.intersection (fs.gitTracked ./.) (
      fs.unions [
        ./site/index.html
        ./site/package.json
        ./site/package-lock.json
        ./site/eslint.config.js
        ./site/tsconfig.json
        ./site/src
        ./site/vite.config.js
      ]
    );
  };

  vmConfig = {
    server = {
      inherit (cfg) vcpu memoryGiB storageTiB;
    };
    billing = {
      inherit (cfg)
        cpuUsdPerVcpuMonth
        memoryUsdPerGibHour
        storageUsdPerTibHour
        marginMultiplier
        ;
    };
  };

  site = ix.buildNpmSite pkgs {
    pname = "resource-monitor-site";
    version = "0.1.0";
    src = siteSrc;
    preBuild = "cp ${pkgs.writeText "resource-monitor-vm-config.json" (builtins.toJSON vmConfig)} src/lib/vm-config.json";
  };

  statsWriter = ix.buildRustPackage pkgs {
    pname = "resource-monitor-stats-writer";
    version = "0.1.0";
    src = ./stats-writer;
    cargoLock.lockFile = ./stats-writer/Cargo.lock;
    meta.mainProgram = "resource-monitor-stats-writer";
  };
in
{
  options.services.resource-monitor = {
    enable = mkEnableOption "browser-accessible VM resource monitor";

    port = mkOption {
      type = types.port;
      default = 80;
      description = "TCP port served by nginx.";
    };

    openFirewall = mkOption {
      type = types.bool;
      default = true;
      description = "Whether to open the nginx port in the in-guest firewall.";
    };

    intervalSeconds = mkOption {
      type = types.ints.positive;
      default = 1;
      description = "Seconds between stats samples.";
    };

    runtimeDirectory = mkOption {
      type = types.str;
      default = "/run/resource-monitor";
      description = "Directory where the generated stats JSON is written.";
    };

    vcpu = mkOption {
      type = types.ints.positive;
      default = 64;
      description = "Advertised total vCPU capacity shown by the monitor.";
    };

    memoryGiB = mkOption {
      type = types.ints.positive;
      default = 256;
      description = "Advertised total memory capacity in GiB.";
    };

    storageTiB = mkOption {
      type = types.ints.positive;
      default = 1024;
      description = "Advertised total storage capacity in TiB.";
    };

    cpuUsdPerVcpuMonth = mkOption {
      type = types.number;
      default = 20;
      description = "CPU billing rate in USD per vCPU-month.";
    };

    memoryUsdPerGibHour = mkOption {
      type = types.number;
      default = 0.005;
      description = "Memory billing rate in USD per GiB-hour.";
    };

    storageUsdPerTibHour = mkOption {
      type = types.number;
      default = 0.0031;
      description = "Storage billing rate in USD per TiB-hour.";
    };

    marginMultiplier = mkOption {
      type = types.number;
      default = 2;
      description = "Multiplier applied to memory and storage billing rates.";
    };
  };

  config = mkIf cfg.enable {
    ix.networking.portClaims.resource-monitor = {
      protocol = "tcp";
      inherit (cfg) port;
      address = "0.0.0.0";
      description = "resource monitor nginx";
    };

    systemd.services.resource-monitor = {
      description = "VM resource monitor stats";
      wantedBy = [ "multi-user.target" ];
      serviceConfig = {
        Type = "simple";
        RuntimeDirectory = builtins.baseNameOf cfg.runtimeDirectory;
        RuntimeDirectoryMode = "0755";
        ExecStart = lib.escapeShellArgs [
          (lib.getExe statsWriter)
          "--output-dir"
          cfg.runtimeDirectory
          "--interval-seconds"
          (toString cfg.intervalSeconds)
          "--total-cores"
          (toString cfg.vcpu)
          "--total-memory-gib"
          (toString cfg.memoryGiB)
          "--total-storage-tib"
          (toString cfg.storageTiB)
          "--cpu-usd-per-vcpu-month"
          (toString cfg.cpuUsdPerVcpuMonth)
          "--memory-usd-per-gib-hour"
          (toString cfg.memoryUsdPerGibHour)
          "--storage-usd-per-tib-hour"
          (toString cfg.storageUsdPerTibHour)
          "--margin-multiplier"
          (toString cfg.marginMultiplier)
          "--df"
          (lib.getExe' pkgs.coreutils "df")
        ];
        Restart = "on-failure";
      };
    };

    services.nginx = {
      enable = true;
      virtualHosts.resource-monitor = {
        default = true;
        listen = [
          {
            addr = "0.0.0.0";
            inherit (cfg) port;
          }
        ];
        root = "${site}/share/resource-monitor-site";
        locations."/stats.json".root = cfg.runtimeDirectory;
        locations."/".tryFiles = "$uri $uri/ /index.html";
      };
    };

    networking.firewall.allowedTCPPorts = lib.optionals cfg.openFirewall [ cfg.port ];
  };
}
