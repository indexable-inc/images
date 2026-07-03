# Apache Spark 3.5 as a standalone cluster, tuned and shipping the
# Gluten + Velox native execution engine by default.
#
# Plain Spark runs its physical operators on the JVM. Gluten offloads them to
# Velox, a vectorized C++ engine; that is the source of the large analytical
# speedups. Enabling it is more than a jar on the classpath: Velox allocates its
# buffers off-heap and needs a columnar shuffle manager, so the generated
# `spark-defaults.conf` wires the plugin, off-heap memory, and columnar shuffle
# together. Turn {option}`services.ix-spark.nativeEngine.enable` off to drop
# back to stock JVM execution.
#
# Topology: ONE Spark cluster over Tailscale. Exactly one node sets
# `role = "master"` (it runs the master daemon and optionally a Spark Connect
# server); every other node is `role = "worker"` and must set `masterAddress`
# to the master's tailscale IPv4. Daemons bind their *tailscale* IPv4 at
# runtime, so the cluster lives on the tailnet, which is also the trust
# boundary.
#
# Repo-agnostic on purpose: declares no `ix.*` NixOS *options* (port-claim /
# health-check bookkeeping), so it imports cleanly into any NixOS system. It
# needs only the index flake lib (`writeNushellApplication`/`systemdHardening`)
# via the `indexLib` arg (see its note below). The spark distribution + Gluten
# bundle are index-overlay packages, so an off-index consumer passes them via
# `package` / `nativeEngine.package` (e.g. `inputs.index.packages.<sys>.spark-hive`).
#
# The Gluten jar is referenced by its absolute store path. A real multi-node
# cluster needs that same store path present on every worker (shared nix store
# or copied closure).
#
# `indexLib` is the index flake lib, supplied by the consumer via
# `_module.args.indexLib` (NOT named `ix`, which a host binds to its own
# specialArg). In index's own eval contexts wire `_module.args.indexLib = ix`;
# elsewhere `_module.args.indexLib = inputs.index.lib`.
{
  config,
  indexLib,
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
    optional
    optionalAttrs
    optionals
    types
    ;

  cfg = config.services.ix-spark;
  dataDir = "/var/lib/spark";

  # `spark-hive` is the official complete distribution (hadoop3 + Hive) pinned to
  # JDK 17. Hive support is mandatory: Gluten's HiveTableScanExecTransformer
  # eagerly loads Hive input-format classes during planning, so the lean
  # nixpkgs `spark` (no hive-exec) cannot initialize it. The package's bin
  # wrappers `--set JAVA_HOME`, so the daemons run on its JDK 17.
  sparkClass = "${cfg.package}/bin/spark-class";

  # Driver/executor JVM options for the native path:
  # - `--add-opens` open the modules Arrow's JNI reflects into on JDK 17.
  # - `io.netty.tryReflectionSetAccessible=true` is required for Arrow's off-heap
  #   allocator: Netty (which Arrow's memory layer uses) refuses the
  #   `DirectByteBuffer(long, int)` constructor on JDK 9+ unless this is set, even
  #   with java.nio opened, surfacing as "sun.misc.Unsafe or
  #   java.nio.DirectByteBuffer.<init>(long, int) not available".
  # - `java.io.tmpdir` is pinned to the on-disk state dir because Gluten extracts
  #   ~270 MiB of native libraries (libvelox.so et al.) per JVM out of the jar at
  #   startup; the default `/tmp` is RAM-backed tmpfs here, so leaving it there
  #   would burn that much RAM per executor on top of the off-heap budget.
  nativeJavaOpts = lib.concatStringsSep " " [
    "-XX:+IgnoreUnrecognizedVMOptions"
    "--add-opens=java.base/java.nio=ALL-UNNAMED"
    "--add-opens=java.base/sun.nio.ch=ALL-UNNAMED"
    "--add-opens=java.base/java.lang=ALL-UNNAMED"
    "-Dio.netty.tryReflectionSetAccessible=true"
    "-Djava.io.tmpdir=${dataDir}/tmp"
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
    "spark.driver.extraJavaOptions" = nativeJavaOpts;
    "spark.executor.extraJavaOptions" = nativeJavaOpts;
  };

  # Tuned defaults; `cfg.settings` is merged over the top so a user key wins.
  # Driver/block-manager ports are pinned so a firewalled tailnet works.
  tunedDefaults = {
    "spark.master" = "spark://__MASTER__:${toString cfg.master.port}";
    "spark.sql.adaptive.enabled" = "true";
    "spark.sql.adaptive.coalescePartitions.enabled" = "true";
    "spark.serializer" = "org.apache.spark.serializer.KryoSerializer";
    "spark.local.dir" = "${dataDir}/local";
    "spark.driver.port" = "7078";
    "spark.driver.blockManager.port" = "7079";
    "spark.blockManager.port" = "7080";
  }
  // nativeSettings;

  finalSettings = tunedDefaults // cfg.settings;

  sparkDefaultsConf = pkgs.writeText "spark-defaults.conf" (
    lib.concatMapAttrsStringSep "" (key: value: "${key} ${toString value}\n") finalSettings
  );

  confDir = pkgs.runCommand "spark-conf" { } ''
    mkdir -p "$out"
    cp ${sparkDefaultsConf} "$out/spark-defaults.conf"
  '';

  # Master/worker run via `spark-class` in the foreground (Type=simple), so their
  # logs go to the journal rather than SPARK_LOG_DIR; no log dir is set. JAVA_HOME
  # is baked into the package's bin wrappers, so it is not set here.
  sparkEnv = {
    SPARK_HOME = "${cfg.package}";
    SPARK_CONF_DIR = "${confDir}";
    SPARK_WORKER_DIR = "${dataDir}/work";
    SPARK_LOCAL_DIRS = "${dataDir}/local";
  };

  # Resolve this node's tailscale IPv4 at runtime (it is host state, not a Nix
  # value), fail loudly if tailscale is not up, then exec the spark daemon bound
  # to it. The token `__IP__` in every argument is substituted with the resolved
  # tailscale address so daemons advertise their tailnet address.
  sparkLauncher = indexLib.writeNushellApplication pkgs {
    name = "ix-spark-launch";
    meta.description = "Resolve this node's tailscale IPv4 and exec a Spark daemon bound to it";
    runtimeInputs = [
      pkgs.tailscale
      cfg.package
    ];
    text = ''
      # nu
      def main [...args: string] {
        let ip = (do --ignore-errors {
          ^tailscale ip -4 | lines | where ($it | str trim | is-not-empty) | first
        } | default "")
        if ($ip | str trim | is-empty) {
          print --stderr "ix-spark: no tailscale IPv4 yet; is tailscaled up?"
          exit 1
        }
        $env.SPARK_LOCAL_IP = $ip
        let resolved = ($args | each { |a| $a | str replace --all "__IP__" $ip })
        exec ...$resolved
      }
    '';
  };

  mkUnit = description: argv: {
    inherit description;
    after = [
      "network-online.target"
      "tailscaled.service"
    ];
    wants = [ "network-online.target" ];
    wantedBy = [ "multi-user.target" ];
    environment = sparkEnv;
    serviceConfig = indexLib.systemdHardening // {
      Type = "simple";
      User = "spark";
      Group = "spark";
      StateDirectory = "spark";
      WorkingDirectory = dataDir;
      ExecStartPre = "${pkgs.coreutils}/bin/mkdir -p ${dataDir}/work ${dataDir}/local ${dataDir}/tmp";
      ExecStart = lib.escapeShellArgs ([ (lib.getExe sparkLauncher) ] ++ argv);
      Restart = "on-failure";
      RestartSec = 5;
    };
  };

  workerCoreArgs = lib.optionals (cfg.worker.cores != null) [
    "--cores"
    (toString cfg.worker.cores)
  ];

  workerMemArgs = lib.optionals (cfg.worker.memory != null) [
    "--memory"
    cfg.worker.memory
  ];
in
{
  options.services.ix-spark = {
    enable = mkEnableOption "Apache Spark standalone cluster with the Gluten/Velox native engine";

    role = mkOption {
      type = types.enum [
        "master"
        "worker"
      ];
      default = "master";
      description = ''
        This node's role in the cluster. A lone node (single-node setup) keeps
        the default `"master"`. Exactly one node must be `"master"` (it runs the
        master daemon and optionally a Spark Connect server); every other node is
        `"worker"` and must set
        {option}`services.ix-spark.masterAddress`.
      '';
    };

    masterAddress = mkOption {
      type = types.nullOr types.str;
      default = null;
      example = "100.64.0.1";
      description = ''
        The master node's tailscale IPv4. Required on workers (they join
        `spark://<masterAddress>:<master.port>`); must be null on the master.
      '';
    };

    package = mkPackageOption pkgs "spark-hive" {
      extraDescription = ''
        The official complete Spark 3.5 distribution (hadoop3 + Hive) pinned to
        JDK 17. Hive support is mandatory for the Gluten native engine, and the
        Gluten Velox bundle in {option}`services.ix-spark.nativeEngine.package`
        is built against Spark 3.5, so keep these versions aligned. Lives in the
        index overlay; an off-index consumer passes
        `inputs.index.packages.<sys>.spark-hive`.
      '';
    };

    master = {
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
        description = "Run a worker on this node, registered with the local master (master role only).";
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

    connect = {
      enable = mkOption {
        type = types.bool;
        default = true;
        description = ''
          Run a Spark Connect gRPC server on the master node. Clients can then
          connect via the Connect protocol without bundling a full Spark
          distribution.
        '';
      };
      port = mkOption {
        type = types.port;
        default = 15002;
        description = "Spark Connect gRPC bind port.";
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
      description = "Open the master RPC, web UI, Connect, and inter-node data ports in the firewall.";
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
    assertions = [
      {
        assertion = cfg.role == "master" -> cfg.masterAddress == null;
        message = "services.ix-spark: the master node must not set masterAddress.";
      }
      {
        assertion = cfg.role == "worker" -> cfg.masterAddress != null;
        message = "services.ix-spark: a worker must set masterAddress to the master's tailscale IP.";
      }
    ];

    # Velox (bundled in the Gluten jar) resolves the IANA tz database through its
    # date library, which hardcodes the FHS `/usr/share/zoneinfo` and ignores
    # $TZDIR. NixOS does not populate that path, so point it at the tzdata store
    # path; without it every Gluten query fails with
    # "discover_tz_dir failed to find zoneinfo". Global (not unit-scoped) so a
    # `spark-submit` driver run outside the service units resolves it too.
    systemd.tmpfiles.rules = [
      "L+ /usr/share/zoneinfo - - - - ${pkgs.tzdata}/share/zoneinfo"
    ];

    users.users.spark = {
      isSystemUser = true;
      group = "spark";
      home = dataDir;
      description = "Apache Spark";
    };
    users.groups.spark = { };

    # Scoped to the tailscale interface, never the global firewall: Spark's
    # master RPC and Connect server carry no authentication (submitting a job
    # is code execution), and a fleet host can also have a PUBLIC interface --
    # a global `allowedTCPPorts` would have exposed them to the internet
    # (index#1800 review, same class as ix-ray). The daemons bind the
    # tailscale IPv4, so that interface is exactly where these ports may open.
    networking.firewall.interfaces."tailscale0" = mkIf cfg.openFirewall {
      allowedTCPPorts =
        # Pinned inter-node data-plane ports (driver + block managers), opened on
        # every node so executors and the driver reach each other over the tailnet.
        [
          7078
          7079
          7080
        ]
        ++ optionals (cfg.role == "master") [
          cfg.master.port
          cfg.master.webUiPort
        ]
        ++ optional (cfg.role == "master" && cfg.connect.enable) cfg.connect.port
        ++ optional cfg.worker.enable cfg.worker.webUiPort;
    };

    systemd.services =
      # --- Master daemon (master role only) ---
      optionalAttrs (cfg.role == "master") {
        spark-master = mkUnit "Apache Spark master" [
          sparkClass
          "org.apache.spark.deploy.master.Master"
          "--host"
          "__IP__"
          "--port"
          (toString cfg.master.port)
          "--webui-port"
          (toString cfg.master.webUiPort)
        ];
      }
      # --- Local worker co-located with master ---
      // optionalAttrs (cfg.role == "master" && cfg.worker.enable) {
        spark-worker =
          mkUnit "Apache Spark worker" (
            [
              sparkClass
              "org.apache.spark.deploy.worker.Worker"
              "spark://__IP__:${toString cfg.master.port}"
              "--host"
              "__IP__"
              "--webui-port"
              (toString cfg.worker.webUiPort)
            ]
            ++ workerCoreArgs
            ++ workerMemArgs
          )
          // {
            after = [
              "network-online.target"
              "tailscaled.service"
              "spark-master.service"
            ];
            requires = [ "spark-master.service" ];
          };
      }
      # --- Spark Connect server (master role only) ---
      # Launched through `start-connect-server.sh` (which wraps spark-submit and
      # puts the bundled `spark-connect_2.12-3.5.x.jar` from the full spark-hive
      # distribution on the classpath, so no `--packages`/network is needed),
      # NOT `spark-class` directly: `spark-class` runs the JVM class and ignores
      # spark-submit args like `--master`/`--conf`, which the connect server needs.
      # SPARK_NO_DAEMONIZE keeps it in the foreground for systemd Type=simple.
      # NOTE: the connect-server runtime path is eval-verified only -- spark-hive
      # is x86_64-linux-only, so a multi-node Spark Connect bring-up cannot be
      # exercised on a darwin dev box and should be confirmed on first deploy.
      // optionalAttrs (cfg.role == "master" && cfg.connect.enable) {
        spark-connect =
          mkUnit "Apache Spark Connect server" [
            "${cfg.package}/sbin/start-connect-server.sh"
            "--master"
            "spark://__IP__:${toString cfg.master.port}"
            "--conf"
            "spark.connect.grpc.binding.host=__IP__"
            "--conf"
            "spark.connect.grpc.binding.port=${toString cfg.connect.port}"
          ]
          // {
            environment = sparkEnv // {
              SPARK_NO_DAEMONIZE = "1";
            };
            after = [
              "network-online.target"
              "tailscaled.service"
              "spark-master.service"
            ];
            requires = [ "spark-master.service" ];
          };
      }
      # --- Remote worker (worker role only) ---
      // optionalAttrs (cfg.role == "worker") {
        spark-worker = mkUnit "Apache Spark worker" (
          [
            sparkClass
            "org.apache.spark.deploy.worker.Worker"
            "spark://${cfg.masterAddress}:${toString cfg.master.port}"
            "--host"
            "__IP__"
            "--webui-port"
            (toString cfg.worker.webUiPort)
          ]
          ++ workerCoreArgs
          ++ workerMemArgs
        );
      };
  };
}
