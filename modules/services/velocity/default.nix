# Velocity Minecraft proxy. https://papermc.io/software/velocity
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
    types
    ;

  cfg = config.services.velocity;
  defaultJvmVersion = import ../../../lib/languages/jvm-defaults.nix;
  yourkit = ix.languages.java.yourkit;
  dataDir = "/var/lib/velocity";
  java = lib.getExe' cfg.javaPackage "java";
  systemctl = lib.getExe' config.systemd.package "systemctl";
  tomlFormat = pkgs.formats.toml { };
  yamlFormat = pkgs.formats.yaml { };
  jsonFormat = pkgs.formats.json { };
  propertiesFormat = pkgs.formats.keyValue { };
  formatValueType = jsonFormat.type;
  fileExt = path: lib.last (lib.splitString "." path);
  hostPort =
    address: port:
    let
      host = if lib.hasInfix ":" address && !lib.hasPrefix "[" address then "[${address}]" else address;
    in
    "${host}:${toString port}";

  pluginType = types.submodule (
    { name, ... }:
    {
      options = {
        enable = mkOption {
          type = types.bool;
          default = true;
          description = "Whether to install this Velocity plugin.";
        };

        src = mkOption {
          type = types.nullOr types.path;
          default = null;
          description = "Plugin jar. Leave unset to resolve the plugin from pluginCatalog by slug.";
        };

        fileName = mkOption {
          type = types.str;
          default = "${name}.jar";
          defaultText = lib.literalMD "the plugin attribute name with `.jar` appended";
          description = "File name used under Velocity's plugins directory.";
        };
      };
    }
  );

  formatFor =
    path:
    {
      json = jsonFormat;
      properties = propertiesFormat;
      toml = tomlFormat;
      yaml = yamlFormat;
      yml = yamlFormat;
    }
    .${fileExt path}
    or (throw "velocity managed files: unsupported extension .${fileExt path} on '${path}'");

  renderedSettings = {
    "config-version" = "2.7";
    bind = "${cfg.address}:${toString cfg.port}";
    inherit (cfg)
      motd
      ;
    "show-max-players" = cfg.showMaxPlayers;
    "online-mode" = cfg.onlineMode;
    "force-key-authentication" = cfg.forceKeyAuthentication;
    "prevent-client-proxy-connections" = cfg.preventClientProxyConnections;
    "player-info-forwarding-mode" = cfg.forwarding.mode;
    "forwarding-secret-file" = "forwarding.secret";
    "announce-forge" = cfg.announceForge;
    "kick-existing-players" = cfg.kickExistingPlayers;
    "ping-passthrough" = cfg.pingPassthrough;
    "sample-players-in-ping" = cfg.samplePlayersInPing;
    "enable-player-address-logging" = cfg.enablePlayerAddressLogging;

    servers = cfg.servers // {
      inherit (cfg) try;
    };

    "forced-hosts" = cfg.forcedHosts;

    advanced = {
      "compression-threshold" = cfg.advanced.compressionThreshold;
      "compression-level" = cfg.advanced.compressionLevel;
      "login-ratelimit" = cfg.advanced.loginRatelimit;
      "connection-timeout" = cfg.advanced.connectionTimeout;
      "read-timeout" = cfg.advanced.readTimeout;
      "haproxy-protocol" = cfg.advanced.haproxyProtocol;
      "tcp-fast-open" = cfg.advanced.tcpFastOpen;
      "bungee-plugin-message-channel" = cfg.advanced.bungeePluginMessageChannel;
      "show-ping-requests" = cfg.advanced.showPingRequests;
      "failover-on-unexpected-server-disconnect" = cfg.advanced.failoverOnUnexpectedServerDisconnect;
      "announce-proxy-commands" = cfg.advanced.announceProxyCommands;
      "log-command-executions" = cfg.advanced.logCommandExecutions;
      "log-player-connections" = cfg.advanced.logPlayerConnections;
      "accepts-transfers" = cfg.advanced.acceptsTransfers;
      "enable-reuse-port" = cfg.advanced.enableReusePort;
      "command-rate-limit" = cfg.advanced.commandRateLimit;
      "forward-commands-if-rate-limited" = cfg.advanced.forwardCommandsIfRateLimited;
      "kick-after-rate-limited-commands" = cfg.advanced.kickAfterRateLimitedCommands;
      "tab-complete-rate-limit" = cfg.advanced.tabCompleteRateLimit;
      "kick-after-rate-limited-tab-completes" = cfg.advanced.kickAfterRateLimitedTabCompletes;
    };

    query = {
      enabled = cfg.query.enable;
      inherit (cfg.query) port map;
      "show-plugins" = cfg.query.showPlugins;
    };
  }
  // cfg.settings;

  configFilePaths = lib.attrNames cfg.configFiles;
  invalidConfigFilePaths = ix.relativePath.unsafe configFilePaths;
  managedConfigFiles = cfg.configFiles // {
    "velocity.toml" = renderedSettings;
  };
  enabledPlugins = lib.filterAttrs (_: plugin: plugin.enable) cfg.plugins;
  pluginJars = lib.mapAttrsToList (
    slug: plugin:
    let
      src =
        if plugin.src != null then
          plugin.src
        else
          (cfg.pluginCatalog.${slug} or (throw "velocity plugin '${slug}' not in pluginCatalog")).src;
    in
    {
      inherit (plugin) fileName;
      path = src;
    }
  ) enabledPlugins;
  pluginFileNames = map (plugin: plugin.fileName) pluginJars;
  invalidPluginFileNames = ix.relativePath.unsafeNames pluginFileNames;
  duplicatePluginFileNames = ix.lists.findDuplicates pluginFileNames;

  mkManaged =
    label: files:
    let
      linkEntry =
        path: value:
        let
          file = (formatFor path).generate (baseNameOf path) value;
          target = ix.relativePath.shellPath "$out" path;
          targetDir = ix.relativePath.shellParent "$out" path;
        in
        ''
          mkdir -p ${targetDir}
          ln -sf ${lib.escapeShellArg file} ${target}
        '';
    in
    pkgs.runCommand "velocity-managed-${label}" { } ''
      mkdir -p "$out"
      ${lib.concatMapAttrsStringSep "\n" linkEntry files}
    '';

  managed = {
    config = mkManaged "config" managedConfigFiles;
    plugins = pkgs.runCommand "velocity-managed-plugins" { } (
      ''
        mkdir -p "$out"
      ''
      + lib.concatMapStringsSep "\n" (plugin: ''
        ln -s ${lib.escapeShellArg plugin.path} ${ix.relativePath.shellPath "$out" plugin.fileName}
      '') pluginJars
    );
  };
  wildcardClientAddresses = [
    "0.0.0.0"
    "::"
    "[::]"
  ];
  velocityProbeAddress =
    if builtins.elem cfg.address wildcardClientAddresses then "127.0.0.1" else cfg.address;
  velocityProbeTarget = hostPort velocityProbeAddress cfg.port;

  installManagedConfigFiles = lib.concatMapStringsSep "\n" (
    path:
    let
      source = "${managed.config}/${path}";
      target = "${dataDir}/${path}";
    in
    "install -Dm0644 ${lib.escapeShellArg source} ${lib.escapeShellArg target}"
  ) (lib.attrNames managedConfigFiles);

  managedPluginManifest = "${dataDir}/.ix-managed-velocity-plugins";
  installManagedPlugins = lib.concatMapStringsSep "\n" (plugin: ''
    ln -sfn ${lib.escapeShellArg "${managed.plugins}/${plugin.fileName}"} ${lib.escapeShellArg "${dataDir}/plugins/${plugin.fileName}"}
    printf '%s\n' ${lib.escapeShellArg plugin.fileName} >> ${lib.escapeShellArg managedPluginManifest}
  '') pluginJars;

  forwardingSecretFile =
    if cfg.forwarding.secret == null then
      null
    else
      pkgs.writeText "velocity-forwarding-secret" cfg.forwarding.secret;
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

  javaArgs = [
    java
    "-XX:MaxRAMPercentage=${toString cfg.maxRAMPercentage}"
  ]
  ++ yourkit.flagsFor pkgs cfg.yourkit
  ++ cfg.jvmFlags
  ++ [
    "-jar"
    "${cfg.package}"
  ];
in
{
  options.services.velocity = {
    enable = mkEnableOption "Velocity Minecraft proxy";

    package = mkOption {
      type = types.package;
      default = ix.artifacts.minecraft.velocityServers."3.4.0-SNAPSHOT".src;
      defaultText = lib.literalExpression ''ix.artifacts.minecraft.velocityServers."3.4.0-SNAPSHOT".src'';
      description = "Velocity proxy jar.";
    };

    javaPackage = mkPackageOption pkgs "temurin-jre-bin-${defaultJvmVersion}" {
      extraDescription = "Used to run Velocity.";
    };

    maxRAMPercentage = mkOption {
      type = types.int;
      default = 75;
      description = "Max heap as a percentage of available system RAM.";
    };

    jvmFlags = mkOption {
      type = types.listOf types.str;
      default = [ ];
      description = "JVM flags used after heap sizing and before -jar.";
    };

    address = mkOption {
      type = types.str;
      default = "0.0.0.0";
      description = "Address Velocity binds for Java clients.";
    };

    port = mkOption {
      type = types.port;
      default = 25565;
      description = "TCP port Velocity binds for Java clients.";
    };

    openFirewall = mkOption {
      type = types.bool;
      default = true;
      description = "Whether to open the Velocity client port in the firewall.";
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

    motd = mkOption {
      type = types.str;
      default = "<#09add3>A Velocity Server";
      description = "MiniMessage MOTD shown in Java clients' server list.";
    };

    health.motdContains = mkOption {
      type = types.listOf types.str;
      default = [ ];
      example = [ "Survival" ];
      description = ''
        Substrings the rendered MOTD must contain for the `velocity-status`
        health check to pass. Velocity renders MiniMessage tags into plain
        text before the SLP response, so pass the plain-text payload here
        (e.g. `[ "Survival" ]` for `<green>Survival</green>`), not the
        MiniMessage source.

        Empty list (the default) probes SLP without asserting MOTD.
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
      description = "Whether Velocity authenticates Java players with Mojang.";
    };

    forceKeyAuthentication = mkOption {
      type = types.bool;
      default = true;
      description = "Whether Velocity enforces Minecraft's public key authentication.";
    };

    preventClientProxyConnections = mkOption {
      type = types.bool;
      default = false;
      description = "Whether Velocity rejects logins whose Mojang auth network differs from the client network.";
    };

    forwarding = {
      mode = mkOption {
        type = types.enum [
          "none"
          "legacy"
          "bungeeguard"
          "modern"
        ];
        default = "modern";
        description = "Player information forwarding mode written to velocity.toml.";
      };

      secret = mkOption {
        type = types.nullOr types.str;
        default = null;
        description = "Inline forwarding secret copied to Velocity's forwarding.secret file. This lands in the Nix store.";
      };

      secretFile = mkOption {
        type = types.nullOr types.str;
        default = null;
        description = "Runtime file copied to Velocity's forwarding.secret file.";
      };
    };

    announceForge = mkOption {
      type = types.bool;
      default = false;
      description = "Whether Velocity announces Forge/FML compatibility.";
    };

    kickExistingPlayers = mkOption {
      type = types.bool;
      default = false;
      description = "Whether duplicate logins kick the current online session.";
    };

    pingPassthrough = mkOption {
      type = types.enum [
        "disabled"
        "mods"
        "description"
        "all"
      ];
      default = "disabled";
      description = "Backend ping data passthrough mode.";
    };

    samplePlayersInPing = mkOption {
      type = types.bool;
      default = false;
      description = "Whether ping responses include a sample of online players.";
    };

    enablePlayerAddressLogging = mkOption {
      type = types.bool;
      default = true;
      description = "Whether Velocity logs player IP addresses.";
    };

    servers = mkOption {
      type = types.attrsOf types.str;
      default = { };
      example.survival = "127.0.0.1:25566";
      description = "Backend servers keyed by Velocity server name.";
    };

    try = mkOption {
      type = types.listOf types.str;
      default = [ ];
      description = "Backend server names Velocity tries when a player joins or is kicked.";
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
        description = "Minimum packet size before Velocity compresses it.";
      };

      compressionLevel = mkOption {
        type = types.int;
        default = -1;
        description = "zlib compression level, or -1 for the default level.";
      };

      loginRatelimit = mkOption {
        type = types.ints.unsigned;
        default = 3000;
        description = "Minimum milliseconds between connections from the same IP.";
      };

      connectionTimeout = mkOption {
        type = types.ints.positive;
        default = 5000;
        description = "Backend connection timeout in milliseconds.";
      };

      readTimeout = mkOption {
        type = types.ints.positive;
        default = 30000;
        description = "Backend read timeout in milliseconds.";
      };

      haproxyProtocol = mkOption {
        type = types.bool;
        default = false;
        description = "Whether Velocity accepts HAProxy PROXY protocol messages.";
      };

      tcpFastOpen = mkOption {
        type = types.bool;
        default = false;
        description = "Whether Velocity enables TCP Fast Open.";
      };

      bungeePluginMessageChannel = mkOption {
        type = types.bool;
        default = true;
        description = "Whether Velocity supports the BungeeCord plugin messaging channel.";
      };

      showPingRequests = mkOption {
        type = types.bool;
        default = false;
        description = "Whether ping requests are logged.";
      };

      failoverOnUnexpectedServerDisconnect = mkOption {
        type = types.bool;
        default = true;
        description = "Whether Velocity fails players over after unexpected backend disconnects.";
      };

      announceProxyCommands = mkOption {
        type = types.bool;
        default = true;
        description = "Whether proxy commands are announced to 1.13+ clients.";
      };

      logCommandExecutions = mkOption {
        type = types.bool;
        default = false;
        description = "Whether Velocity logs player command execution.";
      };

      logPlayerConnections = mkOption {
        type = types.bool;
        default = true;
        description = "Whether Velocity logs player proxy and backend connection events.";
      };

      acceptsTransfers = mkOption {
        type = types.bool;
        default = false;
        description = "Whether Velocity accepts incoming Minecraft transfer packets.";
      };

      enableReusePort = mkOption {
        type = types.bool;
        default = false;
        description = "Whether Velocity enables SO_REUSEPORT.";
      };

      commandRateLimit = mkOption {
        type = types.ints.unsigned;
        default = 50;
        description = "Milliseconds allowed between player commands.";
      };

      forwardCommandsIfRateLimited = mkOption {
        type = types.bool;
        default = true;
        description = "Whether rate-limited commands are forwarded to the backend.";
      };

      kickAfterRateLimitedCommands = mkOption {
        type = types.int;
        default = 0;
        description = "Commands allowed after rate limiting before a kick, or 0 to disable kicks.";
      };

      tabCompleteRateLimit = mkOption {
        type = types.ints.unsigned;
        default = 10;
        description = "Milliseconds allowed between tab completions.";
      };

      kickAfterRateLimitedTabCompletes = mkOption {
        type = types.int;
        default = 0;
        description = "Tab completions allowed after rate limiting before a kick, or 0 to disable kicks.";
      };
    };

    query = {
      enable = mkEnableOption "Velocity GameSpy 4 query listener";

      port = mkOption {
        type = types.port;
        default = 25565;
        description = "UDP port for query responses.";
      };

      openFirewall = mkOption {
        type = types.bool;
        default = true;
        description = "Whether to open the query UDP port in the firewall.";
      };

      map = mkOption {
        type = types.str;
        default = "Velocity";
        description = "Map name reported through query responses.";
      };

      showPlugins = mkOption {
        type = types.bool;
        default = false;
        description = "Whether query responses include Velocity plugins.";
      };
    };

    plugins = mkOption {
      type = types.attrsOf pluginType;
      default = { };
      description = "Velocity plugins installed as jars under the plugins directory. Empty {} resolves a pinned catalog plugin by slug; attrsets with src install a local or private plugin jar.";
    };

    pluginCatalog = mkOption {
      type = types.attrsOf (
        types.submodule {
          freeformType = formatValueType;
          options.src = mkOption {
            type = types.path;
            description = "Plugin jar realized from the generated artifact catalog.";
          };
        }
      );
      default = ix.artifacts.minecraft.velocityPluginCatalog;
      defaultText = lib.literalExpression "ix.artifacts.minecraft.velocityPluginCatalog";
      description = "Slug to locked Velocity plugin artifact mapping. Defaults to the generated catalog from `images/games/minecraft/plugins/velocity/`.";
    };

    configFiles = mkOption {
      type = types.attrsOf formatValueType;
      default = { };
      description = "Managed config files relative to Velocity's data directory. Supported extensions: .json, .properties, .toml, .yaml, .yml.";
    };

    settings = mkOption {
      inherit (tomlFormat) type;
      default = { };
      description = "Raw velocity.toml settings merged over the typed options.";
    };
  };

  config = mkIf cfg.enable {
    assertions = [
      {
        assertion = cfg.forwarding.secret == null || cfg.forwarding.secretFile == null;
        message = "services.velocity.forwarding cannot set both secret and secretFile";
      }
      {
        assertion = !(builtins.hasAttr "velocity.toml" cfg.configFiles);
        message = "services.velocity.configFiles cannot manage velocity.toml; use services.velocity.settings or typed options.";
      }
      {
        assertion = invalidConfigFilePaths == [ ];
        message = "services.velocity.configFiles contains unsafe relative paths: ${lib.concatStringsSep ", " invalidConfigFilePaths}";
      }
      {
        assertion = invalidPluginFileNames == [ ];
        message = "services.velocity.plugins contains unsafe plugin file names: ${lib.concatStringsSep ", " invalidPluginFileNames}";
      }
      {
        assertion = duplicatePluginFileNames == [ ];
        message = "services.velocity.plugins contains duplicate plugin file names: ${lib.concatStringsSep ", " duplicatePluginFileNames}";
      }
    ];

    ix.networking.portClaims = {
      velocity = {
        protocol = "tcp";
        inherit (cfg) port address;
        description = "Velocity Minecraft proxy";
      };
    }
    // lib.optionalAttrs cfg.query.enable {
      velocity-query = {
        protocol = "udp";
        inherit (cfg.query) port;
        description = "Velocity query";
      };
    }
    // yourkit.portClaimFor {
      owner = "velocity";
      cfg = cfg.yourkit;
    };

    networking.firewall.allowedTCPPorts =
      lib.optional cfg.openFirewall cfg.port ++ yourkit.firewallTcpPortsFor cfg.yourkit;
    networking.firewall.allowedUDPPorts = lib.optional (
      cfg.query.enable && cfg.query.openFirewall
    ) cfg.query.port;

    ix.healthChecks = {
      velocity = {
        from = "guest";
        description = "Velocity systemd unit is active";
        command = [
          systemctl
          "is-active"
          "--quiet"
          "velocity.service"
        ];
      };

      velocity-status = {
        from = "guest";
        description =
          "Velocity answers SLP"
          + lib.optionalString (
            cfg.health.motdContains != [ ]
          ) " and the MOTD contains the configured substrings";
        # Probe the actual bind address for concrete listeners, and loopback
        # for wildcard binds. Velocity speaks the standard Java SLP handshake
        # even though it routes traffic to backends, so an SLP success here
        # proves Velocity itself is healthy independent of any individual Paper
        # backend's state.
        command = [
          (lib.getExe ix.packages.mc-probe)
          velocityProbeTarget
        ]
        ++ lib.concatMap (needle: [
          "--motd-contains"
          needle
        ]) cfg.health.motdContains;
      };
    }
    // lib.optionalAttrs cfg.openFirewall {
      velocity-reachable = {
        from = "host";
        requiresIpv4 = true;
        description = "Velocity client port accepts TCP from operator host";
        # Runs on the operator host (not inside the Nix store), so the tool
        # is named, not store-pathed. macOS and normal Linux hosts provide nc.
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

    environment.etc = {
      "velocity/managed-config".source = managed.config;
      "velocity/managed-plugins".source = managed.plugins;
    };

    users.groups.velocity = { };
    users.users.velocity = {
      description = "Velocity service user";
      isSystemUser = true;
      group = "velocity";
      home = dataDir;
    };

    systemd.services.velocity = {
      description = "Velocity Minecraft proxy";
      after = [ "network-online.target" ];
      wants = [ "network-online.target" ];
      wantedBy = [ "multi-user.target" ];
      restartTriggers = [
        managed.config
        managed.plugins
      ]
      ++ lib.optional (forwardingSecretFile != null) forwardingSecretFile;
      preStart = ''
        set -eu

        mkdir -p ${lib.escapeShellArg "${dataDir}/plugins"}

        if [ -f ${lib.escapeShellArg managedPluginManifest} ]; then
          while IFS= read -r plugin; do
            target=${lib.escapeShellArg "${dataDir}/plugins"}/$plugin
            if [ -L "$target" ]; then
              rm -f "$target"
            fi
          done < ${lib.escapeShellArg managedPluginManifest}
        fi

        : > ${lib.escapeShellArg managedPluginManifest}
        ${installManagedPlugins}
        ${installManagedConfigFiles}
        ${installForwardingSecret}
      '';
      serviceConfig = ix.systemdHardening // {
        Type = "simple";
        User = "velocity";
        Group = "velocity";
        WorkingDirectory = dataDir;
        ExecStart = lib.escapeShellArgs javaArgs;
        Restart = "on-failure";
        StateDirectory = "velocity";
      };
    };
  };
}
