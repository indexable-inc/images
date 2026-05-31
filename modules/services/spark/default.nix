# Apache Spark 3.5 as a single-node standalone cluster, tuned and shipping the
# Gluten + Velox native execution engine by default.
#
# Plain Spark runs its physical operators on the JVM. Gluten offloads them to
# Velox, a vectorized C++ engine; that is the source of the large analytical
# speedups. Enabling it is more than a jar on the classpath: Velox allocates its
# buffers off-heap and needs a columnar shuffle manager, so the generated
# `spark-defaults.conf` wires the plugin, off-heap memory, and columnar shuffle
# together. Turn {option}`services.ix-spark.nativeEngine.enable` off to drop
# back to stock JVM execution.
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
    mkPackageOption
    optionalAttrs
    types
    ;

  cfg = config.services.ix-spark;
  dataDir = "/var/lib/spark";
  java = cfg.jrePackage;
  javaHome = java.home or "${java}";
  masterUrl = "spark://${cfg.master.host}:${toString cfg.master.port}";
  sparkClass = "${cfg.package}/bin/spark-class";

  # Arrow's JNI bridge reflects into java.nio on JDK 17; Spark adds its own
  # module opens, these cover the Arrow/Gluten path specifically.
  arrowOpens = lib.concatStringsSep " " [
    "-XX:+IgnoreUnrecognizedVMOptions"
    "--add-opens=java.base/java.nio=ALL-UNNAMED"
    "--add-opens=java.base/sun.nio.ch=ALL-UNNAMED"
    "--add-opens=java.base/java.lang=ALL-UNNAMED"
  ];

  nativeSettings = optionalAttrs cfg.nativeEngine.enable {
    "spark.plugins" = "org.apache.gluten.GlutenPlugin";
    "spark.gluten.enabled" = "true";
    "spark.gluten.sql.columnar.backend.lib" = "velox";
    "spark.shuffle.manager" = "org.apache.spark.shuffle.sort.ColumnarShuffleManager";
    "spark.memory.offHeap.enabled" = "true";
    "spark.memory.offHeap.size" = cfg.nativeEngine.offHeapSize;
    "spark.driver.extraClassPath" = cfg.nativeEngine.package.jar;
    "spark.executor.extraClassPath" = cfg.nativeEngine.package.jar;
    "spark.driver.extraJavaOptions" = arrowOpens;
    "spark.executor.extraJavaOptions" = arrowOpens;
  };

  # Tuned defaults; `cfg.settings` is merged over the top so a user key wins.
  tunedDefaults = {
    "spark.master" = masterUrl;
    "spark.sql.adaptive.enabled" = "true";
    "spark.sql.adaptive.coalescePartitions.enabled" = "true";
    "spark.serializer" = "org.apache.spark.serializer.KryoSerializer";
    "spark.local.dir" = "${dataDir}/local";
  }
  // nativeSettings;

  finalSettings = tunedDefaults // cfg.settings;

  sparkDefaultsConf = pkgs.writeText "spark-defaults.conf" (
    lib.concatStringsSep "\n" (
      lib.mapAttrsToList (key: value: "${key} ${toString value}") finalSettings
    )
    + "\n"
  );

  confDir = pkgs.runCommand "spark-conf" { } ''
    mkdir -p "$out"
    cp ${sparkDefaultsConf} "$out/spark-defaults.conf"
  '';

  sparkEnv = {
    JAVA_HOME = javaHome;
    SPARK_HOME = "${cfg.package}";
    SPARK_CONF_DIR = "${confDir}";
    SPARK_LOG_DIR = "${dataDir}/logs";
    SPARK_WORKER_DIR = "${dataDir}/work";
    SPARK_LOCAL_DIRS = "${dataDir}/local";
  };

  mkUnit = description: execStart: {
    inherit description;
    after = [ "network-online.target" ];
    wants = [ "network-online.target" ];
    wantedBy = [ "multi-user.target" ];
    environment = sparkEnv;
    serviceConfig = ix.systemdHardening // {
      Type = "simple";
      User = "spark";
      Group = "spark";
      StateDirectory = "spark";
      WorkingDirectory = dataDir;
      ExecStartPre = "${pkgs.coreutils}/bin/mkdir -p ${dataDir}/logs ${dataDir}/work ${dataDir}/local";
      ExecStart = execStart;
      Restart = "on-failure";
      RestartSec = 5;
    };
  };
in
{
  options.services.ix-spark = {
    enable = mkEnableOption "Apache Spark standalone cluster with the Gluten/Velox native engine";

    package = mkOption {
      type = types.package;
      default = pkgs.spark_3_5;
      defaultText = lib.literalExpression "pkgs.spark_3_5";
      description = ''
        Spark distribution. Pinned to the 3.5 line because the Gluten Velox
        bundle in {option}`services.ix-spark.nativeEngine.package` is built
        against Spark 3.5; pairing it with another major fails to load the
        plugin.
      '';
    };

    jrePackage = mkOption {
      type = types.package;
      default = pkgs.jdk17_headless;
      defaultText = lib.literalExpression "pkgs.jdk17_headless";
      description = ''
        JDK/JRE used to run the master and workers. Gluten 1.6 with Spark 3.5 is
        tested on JDK 17; other majors are not covered upstream.
      '';
    };

    master = {
      host = mkOption {
        type = types.str;
        default = "127.0.0.1";
        description = "Address the master binds and workers connect to.";
      };
      port = mkOption {
        type = types.port;
        default = 7077;
        description = "Master RPC port.";
      };
      webUiPort = mkOption {
        type = types.port;
        default = 8080;
        description = "Master web UI port.";
      };
    };

    worker = {
      enable = mkOption {
        type = types.bool;
        default = true;
        description = "Run a worker on this node, registered with the local master.";
      };
      webUiPort = mkOption {
        type = types.port;
        default = 8081;
        description = "Worker web UI port.";
      };
      cores = mkOption {
        type = types.nullOr types.ints.positive;
        default = null;
        description = "Cores the worker offers. Null lets Spark use every core it sees.";
      };
      memory = mkOption {
        type = types.nullOr types.str;
        default = null;
        example = "8g";
        description = "Memory the worker offers (Spark size string). Null lets Spark autodetect.";
      };
    };

    nativeEngine = {
      enable = mkOption {
        type = types.bool;
        default = true;
        description = ''
          Offload execution to Velox via Apache Gluten. This is the point of the
          module; turn it off only to compare against stock JVM execution or to
          isolate a Gluten-specific issue.
        '';
      };
      package = mkPackageOption pkgs "spark-gluten" { };
      offHeapSize = mkOption {
        type = types.str;
        default = "2g";
        description = ''
          Off-heap memory granted to Velox (`spark.memory.offHeap.size`). Velox
          allocates its columnar buffers here rather than on the JVM heap, so
          this is the main native-engine memory knob. Size it together with
          executor heap so both fit the machine.
        '';
      };
    };

    openFirewall = mkOption {
      type = types.bool;
      default = false;
      description = "Open the master RPC and web UI ports in the firewall.";
    };

    settings = mkOption {
      type = types.attrsOf (
        types.oneOf [
          types.str
          types.int
          types.bool
        ]
      );
      default = { };
      example = lib.literalExpression ''{ "spark.sql.shuffle.partitions" = 64; }'';
      description = ''
        Extra `spark-defaults.conf` entries, merged over the tuned defaults this
        module sets. Your keys win, so this is also how you override a tuned
        default.
      '';
    };
  };

  config = mkIf cfg.enable {
    users.users.spark = {
      isSystemUser = true;
      group = "spark";
      home = dataDir;
      description = "Apache Spark";
    };
    users.groups.spark = { };

    ix.networking.portClaims = {
      ix-spark-master = {
        protocol = "tcp";
        inherit (cfg.master) port;
        description = "Spark master RPC";
      };
      ix-spark-master-ui = {
        protocol = "tcp";
        port = cfg.master.webUiPort;
        description = "Spark master web UI";
      };
    }
    // optionalAttrs cfg.worker.enable {
      ix-spark-worker-ui = {
        protocol = "tcp";
        port = cfg.worker.webUiPort;
        description = "Spark worker web UI";
      };
    };

    networking.firewall.allowedTCPPorts = lib.optionals cfg.openFirewall [
      cfg.master.port
      cfg.master.webUiPort
    ];

    ix.healthChecks.ix-spark = {
      from = "guest";
      description = "Spark master web UI responds";
      command = [
        (lib.getExe' pkgs.curl "curl")
        "--fail"
        "--silent"
        "--show-error"
        "--max-time"
        "5"
        "http://${cfg.master.host}:${toString cfg.master.webUiPort}/"
      ];
    };

    systemd.services = {
      spark-master = mkUnit "Apache Spark master" (
        lib.escapeShellArgs [
          sparkClass
          "org.apache.spark.deploy.master.Master"
          "--host"
          cfg.master.host
          "--port"
          (toString cfg.master.port)
          "--webui-port"
          (toString cfg.master.webUiPort)
        ]
      );
    }
    // optionalAttrs cfg.worker.enable {
      spark-worker =
        mkUnit "Apache Spark worker" (
          lib.escapeShellArgs (
            [
              sparkClass
              "org.apache.spark.deploy.worker.Worker"
              masterUrl
              "--webui-port"
              (toString cfg.worker.webUiPort)
            ]
            ++ lib.optionals (cfg.worker.cores != null) [
              "--cores"
              (toString cfg.worker.cores)
            ]
            ++ lib.optionals (cfg.worker.memory != null) [
              "--memory"
              cfg.worker.memory
            ]
          )
        )
        // {
          after = [
            "network-online.target"
            "spark-master.service"
          ];
          requires = [ "spark-master.service" ];
        };
    };
  };
}
