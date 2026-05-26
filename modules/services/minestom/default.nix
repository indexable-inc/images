# Minestom server runtime.
#
# Runs a user-built fat jar. Unlike the Minecraft module, there are no loaders,
# mods, or EULA: Minestom is a from-scratch server library.
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
  cfg = config.services.minestom;
  yourkit = ix.languages.java.yourkit;

  dataDir = "/var/lib/minestom";
  java = lib.getExe' cfg.javaPackage "java";

  javaArgs = [
    java
    "-XX:MaxRAMPercentage=${toString cfg.maxRAMPercentage}"
  ]
  ++ yourkit.flagsFor pkgs cfg.yourkit
  ++ cfg.jvmFlags
  ++ [
    "-jar"
    "${cfg.serverJar}"
  ];
in
{
  options.services.minestom = {
    enable = mkEnableOption "Minestom server";

    serverJar = mkOption {
      type = types.package;
      description = "Fat jar to launch. Built from a Gradle/Maven project that depends on Minestom.";
    };

    maxRAMPercentage = mkOption {
      type = types.int;
      default = 85;
    };

    javaPackage = mkOption {
      type = types.package;
      default = pkgs.temurin-jre-bin-25;
    };

    jvmFlags = mkOption {
      type = types.listOf types.str;
      default = [
        # ZGC for sub-millisecond, heap-size-independent pauses: a single
        # G1 pause near MaxGCPauseMillis (~200 ms in Aikar's flags used by
        # the vanilla Minecraft module) drops multiple 20 TPS server ticks.
        # `+UseZGC` selects generational ZGC by default since JDK 23 and
        # is the only ZGC mode left on the temurin-jre-bin-25 default JRE.
        #   JEP 474 (default mode):   https://openjdk.org/jeps/474
        #   JEP 490 (mode removed):   https://openjdk.org/jeps/490
        #   Oracle GC tuning (JDK 25): https://docs.oracle.com/en/java/javase/25/gctuning/z-garbage-collector.html
        "-XX:+UseZGC"
      ];
    };

    port = mkOption {
      type = types.port;
      default = 25565;
    };

    openFirewall = mkOption {
      type = types.bool;
      default = true;
      description = "Whether to open the Minestom client port in the firewall.";
    };

    yourkit = mkOption {
      type = ix.languages.java.yourkit.type;
      default = { };
      description = ''
        YourKit profiler agent. Enable to load `libyjpagent` at JVM
        startup so call counts and allocations are accurate from the
        first instruction. See [`ix.languages.java.yourkit`](../../lib/languages/java/yourkit.nix)
        for option semantics.
      '';
    };
  };

  config = mkIf cfg.enable {
    ix.networking.portClaims = {
      minestom = {
        protocol = "tcp";
        inherit (cfg) port;
        description = "Minestom server";
      };
    }
    // yourkit.portClaimFor {
      owner = "minestom";
      cfg = cfg.yourkit;
    };

    networking.firewall.allowedTCPPorts =
      lib.optionals cfg.openFirewall [ cfg.port ] ++ yourkit.firewallTcpPortsFor cfg.yourkit;

    systemd.services.minestom = {
      description = "Minestom server";
      after = [ "network-online.target" ];
      wants = [ "network-online.target" ];
      wantedBy = [ "multi-user.target" ];
      serviceConfig = ix.systemdHardening // {
        Type = "simple";
        WorkingDirectory = dataDir;
        ExecStart = lib.escapeShellArgs javaArgs;
        Restart = "on-failure";
        StateDirectory = "minestom";
      };
    };
  };
}
