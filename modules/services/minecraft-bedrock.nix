# Minecraft Bedrock Dedicated Server.
#
# Bedrock is a native Linux server, so it stays separate from the Java
# `services.minecraft` loader family.
{
  config,
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

  version = "1.26.14.1";

  bedrockServer = pkgs.stdenv.mkDerivation {
    pname = "minecraft-bedrock-server";
    inherit version;

    src = pkgs.fetchurl {
      url = "https://www.minecraft.net/bedrockdedicatedserver/bin-linux/bedrock-server-${version}.zip";
      hash = "sha256-g9XaCRI8PwtgPFS+kpaOXA5DdbWE1RTWEID2Nuekx3Q=";
      curlOptsList = [
        "--http1.1"
        "-A"
        "Mozilla/5.0"
      ];
    };

    strictDeps = true;
    # The bedrock zip has no wrapper directory: files land directly in $PWD.
    # Without this, Nix's unpackPhase tries to auto-detect a single extracted
    # directory to cd into, and fails because it finds multiple entries instead.
    sourceRoot = ".";
    nativeBuildInputs = [
      pkgs.autoPatchelfHook
      pkgs.unzip
    ];
    buildInputs = [
      pkgs.curl
      pkgs.glibc
      pkgs.stdenv.cc.cc.lib
    ];
    dontConfigure = true;
    dontBuild = true;

    installPhase = ''
      runHook preInstall

      mkdir -p "$out/bin" "$out/share/minecraft-bedrock-server"
      cp -R . "$out/share/minecraft-bedrock-server/"
      chmod +x "$out/share/minecraft-bedrock-server/bedrock_server"
      ln -s "$out/share/minecraft-bedrock-server/bedrock_server" "$out/bin/bedrock_server"

      runHook postInstall
    '';

    meta.mainProgram = "bedrock_server";
  };

  cfg = config.services.minecraft-bedrock;
  dataDir = "/var/lib/minecraft-bedrock";
  jsonFormat = pkgs.formats.json { };
  propertiesFormat = pkgs.formats.keyValue { };

  propertiesFile = propertiesFormat.generate "server.properties" cfg.settings;
  allowlistFile = jsonFormat.generate "allowlist.json" cfg.allowlist;
  permissionsFile = jsonFormat.generate "permissions.json" cfg.permissions;

  staticEntries = [
    "bedrock_server_how_to.html"
    "behavior_packs"
    "config"
    "data"
    "definitions"
    "packetlimitconfig.json"
    "profanity_filter.wlist"
    "release-notes.txt"
    "resource_packs"
  ];

  staticLinks = lib.concatMapStringsSep "\n" (
    entry:
    let
      source = "${cfg.package}/share/minecraft-bedrock-server/${entry}";
      target = "${dataDir}/${entry}";
    in
    ''
      if [ -L ${lib.escapeShellArg target} ]; then
        ln -sfnT ${lib.escapeShellArg source} ${lib.escapeShellArg target}
      elif [ ! -e ${lib.escapeShellArg target} ]; then
        ln -sT ${lib.escapeShellArg source} ${lib.escapeShellArg target}
      fi
    ''
  ) staticEntries;
in
{
  options.services.minecraft-bedrock = {
    enable = mkEnableOption "Minecraft Bedrock Dedicated Server";

    package = mkOption {
      type = types.package;
      default = bedrockServer;
      description = "Bedrock Dedicated Server package to run.";
    };

    port = mkOption {
      type = types.port;
      default = 19132;
      description = "IPv4 UDP port for Bedrock clients.";
    };

    portv6 = mkOption {
      type = types.port;
      default = 19133;
      description = "IPv6 UDP port for Bedrock clients.";
    };

    settings = mkOption {
      inherit (propertiesFormat) type;
      default = { };
      description = "server.properties values for Bedrock Dedicated Server.";
    };

    allowlist = mkOption {
      inherit (jsonFormat) type;
      default = [ ];
      description = "allowlist.json content.";
    };

    permissions = mkOption {
      inherit (jsonFormat) type;
      default = [ ];
      description = "permissions.json content.";
    };
  };

  config = mkIf cfg.enable {
    services.minecraft-bedrock.settings = {
      server-port = lib.mkDefault cfg.port;
      server-portv6 = lib.mkDefault cfg.portv6;
      enable-lan-visibility = lib.mkDefault false;
    };

    networking.firewall.allowedUDPPorts = [
      cfg.port
      cfg.portv6
    ];

    systemd.services.minecraft-bedrock = {
      description = "Minecraft Bedrock server";
      after = [ "network-online.target" ];
      wants = [ "network-online.target" ];
      wantedBy = [ "multi-user.target" ];
      serviceConfig = {
        Type = "simple";
        WorkingDirectory = dataDir;
        ExecStart = lib.getExe cfg.package;
        Restart = "on-failure";
        StateDirectory = "minecraft-bedrock";
        KillSignal = "SIGINT";
        TimeoutStopSec = 30;

        CapabilityBoundingSet = [ "" ];
        DeviceAllow = [ "" ];
        LockPersonality = true;
        PrivateDevices = true;
        PrivateTmp = true;
        PrivateUsers = true;
        ProtectClock = true;
        ProtectControlGroups = true;
        ProtectHome = true;
        ProtectHostname = true;
        ProtectKernelLogs = true;
        ProtectKernelModules = true;
        ProtectKernelTunables = true;
        ProtectProc = "invisible";
        RestrictAddressFamilies = [
          "AF_INET"
          "AF_INET6"
          "AF_UNIX"
        ];
        RestrictNamespaces = true;
        RestrictRealtime = true;
        RestrictSUIDSGID = true;
        SystemCallArchitectures = "native";
        UMask = "0077";
      };
      preStart = ''
        mkdir -p ${dataDir}/worlds
        ${staticLinks}
        install -m 0644 ${propertiesFile} ${dataDir}/server.properties
        install -m 0644 ${allowlistFile} ${dataDir}/allowlist.json
        install -m 0644 ${permissionsFile} ${dataDir}/permissions.json
      '';
    };
  };
}
