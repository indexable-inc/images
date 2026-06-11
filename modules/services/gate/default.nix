# Gate Minecraft proxy. https://gate.minekube.com/
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

  cfg = config.services.gate;
  dataDir = "/var/lib/gate";
  gate = lib.getExe cfg.package;
  systemctl = lib.getExe' config.systemd.package "systemctl";
  yamlFormat = pkgs.formats.yaml { };

  hostPort =
    address: port:
    let
      host = if lib.hasInfix ":" address && !lib.hasPrefix "[" address then "[${address}]" else address;
    in
    "${host}:${toString port}";

  # Gate config is rooted under a `config:` key. Typed options below render
  # to that subtree; cfg.settings is merged over the typed result so a caller
  # can reach any key the typed surface does not cover.
  renderedSettings = {
    config = {
      bind = hostPort cfg.address cfg.port;
      onlineMode = cfg.onlineMode;
      forceKeyAuthentication = cfg.forceKeyAuthentication;
      acceptTransfers = cfg.acceptTransfers;
      bungeePluginChannelEnabled = cfg.bungeePluginChannelEnabled;
      announceProxyCommands = cfg.advanced.announceProxyCommands;

      servers = cfg.servers;
      try = cfg.try;
      forcedHosts = cfg.forcedHosts;

      status = {
        motd = cfg.motd;
        showMaxPlayers = cfg.showMaxPlayers;
        logPingRequests = cfg.advanced.logPingRequests;
        announceForge = cfg.announceForge;
      };

      compression = {
        threshold = cfg.advanced.compressionThreshold;
        level = cfg.advanced.compressionLevel;
      };

      connectionTimeout = cfg.advanced.connectionTimeout;
      readTimeout = cfg.advanced.readTimeout;
      failoverOnUnexpectedServerDisconnect = cfg.advanced.failoverOnUnexpectedServerDisconnect;
    }
    // lib.optionalAttrs (cfg.forwarding.mode != "none") {
      forwarding = {
        mode = cfg.forwarding.mode;
      };
    };
  }
  // cfg.settings;

  configFile = yamlFormat.generate "gate-config.yml" renderedSettings;

  forwardingSecretFile =
    if cfg.forwarding.secret == null then
      null
    else
      pkgs.writeText "gate-forwarding-secret" cfg.forwarding.secret;
  installForwardingSecret =
    if cfg.forwarding.secret != null then
      "install -Dm0600 ${lib.escapeShellArg forwardingSecretFile} ${lib.escapeShellArg "${dataDir}/forwarding.secret"}"
    else if cfg.forwarding.secretFile != null then
      "install -Dm0600 ${lib.escapeShellArg cfg.forwarding.secretFile} ${lib.escapeShellArg "${dataDir}/forwarding.secret"}"
    else
      ''
        if [ ! -s ${lib.escapeShellArg "${dataDir}/forwarding.secret"} ]; then
          ${lib.getExe pkgs.openssl} rand -base64 32 > ${lib.escapeShellArg "${dataDir}/forwarding.secret"}
          chmod 0600 ${lib.escapeShellArg "${dataDir}/forwarding.secret"}
        fi
      '';

  wildcardClientAddresses = [
    "0.0.0.0"
    "::"
    "[::]"
  ];
  gateProbeAddress =
    if builtins.elem cfg.address wildcardClientAddresses then "127.0.0.1" else cfg.address;
  gateProbeTarget = hostPort gateProbeAddress cfg.port;
in
{
  options.services.gate = {
    enable = mkEnableOption "Gate Minecraft proxy";

    package = mkOption {
      type = types.package;
      default = ix.packages.gate;
      defaultText = lib.literalExpression "ix.packages.gate";
      description = "Gate proxy binary.";
    };

    address = mkOption {
      type = types.str;
      default = "0.0.0.0";
      description = "Address Gate binds for Java clients.";
    };

    port = mkOption {
      type = types.port;
      default = 25565;
      description = "TCP port Gate binds for Java clients.";
    };

    openFirewall = mkOption {
      type = types.bool;
      default = true;
      description = "Whether to open the Gate client port in the firewall.";
    };

    motd = mkOption {
      type = types.str;
      default = "§bA Gate Proxy";
      description = "MOTD shown in Java clients' server list. Accepts legacy '§' format or a modern text-component JSON string.";
    };

    health.motdContains = mkOption {
      type = types.listOf types.str;
      default = [ ];
      example = [ "Survival" ];
      description = ''
        Substrings the rendered MOTD must contain for the `gate-status`
        health check to pass. Empty list (the default) probes SLP without
        asserting MOTD.
      '';
    };

    showMaxPlayers = mkOption {
      type = types.ints.positive;
      default = 500;
      description = "Displayed maximum player count in the server list.";
    };

    onlineMode = mkOption {
      type = types.bool;
      default = true;
      description = "Whether Gate authenticates Java players with Mojang.";
    };

    forceKeyAuthentication = mkOption {
      type = types.bool;
      default = true;
      description = "Whether Gate enforces Minecraft's public key authentication.";
    };

    acceptTransfers = mkOption {
      type = types.bool;
      default = false;
      description = "Whether Gate accepts incoming Minecraft transfer packets (1.20.5+).";
    };

    bungeePluginChannelEnabled = mkOption {
      type = types.bool;
      default = true;
      description = "Whether Gate supports the BungeeCord plugin messaging channel.";
    };

    announceForge = mkOption {
      type = types.bool;
      default = false;
      description = "Whether Gate announces Forge/FML compatibility.";
    };

    forwarding = {
      mode = mkOption {
        type = types.enum [
          "none"
          "legacy"
          "bungeeguard"
          "velocity"
        ];
        default = "velocity";
        description = "Player information forwarding mode written to Gate's config.";
      };

      secret = mkOption {
        type = types.nullOr types.str;
        default = null;
        description = "Inline forwarding secret copied to Gate's forwarding.secret file. This lands in the Nix store.";
      };

      secretFile = mkOption {
        type = types.nullOr types.str;
        default = null;
        description = "Runtime file copied to Gate's forwarding.secret file.";
      };
    };

    servers = mkOption {
      type = types.attrsOf types.str;
      default = { };
      example.survival = "127.0.0.1:25566";
      description = "Backend servers keyed by Gate server name.";
    };

    try = mkOption {
      type = types.listOf types.str;
      default = [ ];
      description = "Backend server names Gate tries when a player joins or is kicked.";
    };

    forcedHosts = mkOption {
      type = types.attrsOf (types.listOf types.str);
      default = { };
      description = "Host name to backend server order mapping.";
    };

    advanced = {
      compressionThreshold = mkOption {
        type = types.int;
        default = 256;
        description = "Minimum packet size before Gate compresses it.";
      };

      compressionLevel = mkOption {
        type = types.int;
        default = -1;
        description = "zlib compression level, or -1 for the default level.";
      };

      connectionTimeout = mkOption {
        type = types.str;
        default = "5s";
        description = "Backend connection timeout (Go duration string).";
      };

      readTimeout = mkOption {
        type = types.str;
        default = "30s";
        description = "Backend read timeout (Go duration string).";
      };

      failoverOnUnexpectedServerDisconnect = mkOption {
        type = types.bool;
        default = true;
        description = "Whether Gate fails players over after unexpected backend disconnects.";
      };

      announceProxyCommands = mkOption {
        type = types.bool;
        default = true;
        description = "Whether proxy commands are announced to 1.13+ clients.";
      };

      logPingRequests = mkOption {
        type = types.bool;
        default = false;
        description = "Whether ping requests are logged.";
      };
    };

    settings = mkOption {
      inherit (yamlFormat) type;
      default = { };
      description = "Raw Gate config merged over the typed options. Top-level key is `config:`.";
    };
  };

  config = mkIf cfg.enable {
    assertions = [
      {
        assertion = cfg.forwarding.secret == null || cfg.forwarding.secretFile == null;
        message = "services.gate.forwarding cannot set both secret and secretFile";
      }
    ];

    ix.networking.portClaims.gate = {
      protocol = "tcp";
      inherit (cfg) port address;
      description = "Gate Minecraft proxy";
    };

    networking.firewall.allowedTCPPorts = lib.optional cfg.openFirewall cfg.port;

    ix.healthChecks = {
      gate = {
        from = "guest";
        description = "Gate systemd unit is active";
        command = [
          systemctl
          "is-active"
          "--quiet"
          "gate.service"
        ];
      };

      gate-status = {
        from = "guest";
        description =
          "Gate answers SLP"
          + lib.optionalString (
            cfg.health.motdContains != [ ]
          ) " and the MOTD contains the configured substrings";
        # Gate speaks the standard Java SLP handshake even though it routes
        # traffic to backends, so an SLP success here proves Gate itself is
        # healthy independent of any individual Paper backend's state.
        command = [
          (lib.getExe ix.packages.mc-probe)
          gateProbeTarget
        ]
        ++ lib.concatMap (needle: [
          "--motd-contains"
          needle
        ]) cfg.health.motdContains;
      };
    }
    // lib.optionalAttrs cfg.openFirewall {
      gate-reachable = {
        from = "host";
        requiresIpv4 = true;
        description = "Gate client port accepts TCP from operator host";
        command = [
          "nc"
          "-z"
          "-w"
          "5"
          "$IX_NODE_IPV4"
          (toString cfg.port)
        ];
      };
    };

    environment.systemPackages = [ ix.packages.mc-probe ];

    environment.etc."gate/config.yml".source = configFile;

    users.groups.gate = { };
    users.users.gate = {
      description = "Gate service user";
      isSystemUser = true;
      group = "gate";
      home = dataDir;
    };

    systemd.services.gate = {
      description = "Gate Minecraft proxy";
      after = [ "network-online.target" ];
      wants = [ "network-online.target" ];
      wantedBy = [ "multi-user.target" ];
      restartTriggers = [
        configFile
      ]
      ++ lib.optional (forwardingSecretFile != null) forwardingSecretFile;
      preStart = ''
        set -eu
        install -Dm0644 ${lib.escapeShellArg configFile} ${lib.escapeShellArg "${dataDir}/config.yml"}
        ${installForwardingSecret}
      '';
      serviceConfig = ix.systemdHardening // {
        Type = "simple";
        User = "gate";
        Group = "gate";
        WorkingDirectory = dataDir;
        ExecStart = lib.escapeShellArgs [
          gate
          "--config"
          "${dataDir}/config.yml"
        ];
        Restart = "on-failure";
        StateDirectory = "gate";
      };
    };
  };
}
