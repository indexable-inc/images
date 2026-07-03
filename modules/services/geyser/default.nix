# Geyser Bedrock-to-Java bridge, installed as a Velocity plugin.
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
    types
    ;

  cfg = config.services.geyser;
  formatValueType = (pkgs.formats.json {}).type;
  velocityPluginPath = "plugins/geyser/config.yml";

  renderedConfig =
    {
      bedrock = {
        inherit
          (cfg.bedrock)
          address
          motd1
          motd2
          port
          ;
        "clone-remote-port" = cfg.bedrock.cloneRemotePort;
        "server-name" = cfg.bedrock.serverName;
        "compression-level" = cfg.bedrock.compressionLevel;
        "enable-proxy-protocol" = cfg.bedrock.enableProxyProtocol;
      };

      remote = {
        inherit (cfg.remote) address port;
        "auth-type" = cfg.remote.authType;
        "use-proxy-protocol" = cfg.remote.useProxyProtocol;
        "forward-hostname" = cfg.remote.forwardHostname;
      };

      "floodgate-key-file" = cfg.floodgateKeyFile;
      "command-suggestions" = cfg.commandSuggestions;
      "passthrough-motd" = cfg.passthrough.motd;
      "passthrough-player-counts" = cfg.passthrough.playerCounts;
      "legacy-ping-passthrough" = cfg.passthrough.legacyPing;
      "ping-passthrough-interval" = cfg.passthrough.interval;
      "forward-player-ping" = cfg.forwardPlayerPing;
      "max-players" = cfg.maxPlayers;
      "debug-mode" = cfg.debug;
      "show-cooldown" = cfg.showCooldown;
      "show-coordinates" = cfg.showCoordinates;
      "disable-bedrock-scaffolding" = cfg.disableBedrockScaffolding;
      "emote-offhand-workaround" = cfg.emoteOffhandWorkaround;
      "cache-images" = cfg.cacheImages;
      "allow-custom-skulls" = cfg.allowCustomSkulls;
      "max-visible-custom-skulls" = cfg.maxVisibleCustomSkulls;
      "custom-skull-render-distance" = cfg.customSkullRenderDistance;
      "add-non-bedrock-items" = cfg.addNonBedrockItems;
      "above-bedrock-nether-building" = cfg.aboveBedrockNetherBuilding;
      "force-resource-packs" = cfg.forceResourcePacks;
      "xbox-achievements-enabled" = cfg.xboxAchievements;
      "log-player-ip-addresses" = cfg.logPlayerIpAddresses;
      "notify-on-new-bedrock-update" = cfg.notifyOnNewBedrockUpdate;
      "unusable-space-block" = cfg.unusableSpaceBlock;
      metrics.enabled = cfg.metrics.enable;
      "scoreboard-packet-threshold" = cfg.scoreboardPacketThreshold;
      "enable-proxy-connections" = cfg.enableProxyConnections;
      inherit (cfg) mtu;
      "use-direct-connection" = cfg.useDirectConnection;
      "disable-compression" = cfg.disableCompression;
      "config-version" = 4;
    }
    // cfg.settings;
in {
  options.services.geyser = {
    enable = mkEnableOption "Geyser Bedrock-to-Java bridge";

    platform = mkOption {
      type = types.enum ["velocity"];
      default = "velocity";
      description = "Platform integration used for Geyser.";
    };

    package = mkOption {
      type = types.package;
      default = ix.artifacts.minecraft.velocityPluginCatalog.geyser-velocity.src;
      defaultText = lib.literalExpression "ix.artifacts.minecraft.velocityPluginCatalog.geyser-velocity.src";
      description = "Geyser Velocity plugin jar.";
    };

    bedrock = {
      address = mkOption {
        type = types.str;
        default = "0.0.0.0";
        description = "Address Geyser binds for Bedrock clients.";
      };

      port = mkOption {
        type = types.port;
        default = 19132;
        description = "UDP port Geyser binds for Bedrock clients.";
      };

      openFirewall = mkOption {
        type = types.bool;
        default = true;
        description = "Whether to open the Bedrock UDP port in the firewall.";
      };

      cloneRemotePort = mkOption {
        type = types.bool;
        default = false;
        description = "Whether plugin Geyser mirrors the Java listener port for Bedrock.";
      };

      motd1 = mkOption {
        type = types.str;
        default = "Geyser";
        description = "First Bedrock MOTD line.";
      };

      motd2 = mkOption {
        type = types.str;
        default = "Another Geyser server.";
        description = "Second Bedrock MOTD line.";
      };

      serverName = mkOption {
        type = types.str;
        default = "Geyser";
        description = "Server name shown in Bedrock menus.";
      };

      compressionLevel = mkOption {
        type = types.int;
        default = 6;
        description = "Bedrock packet compression level.";
      };

      enableProxyProtocol = mkOption {
        type = types.bool;
        default = false;
        description = "Whether Geyser accepts UDP PROXY protocol on Bedrock connections.";
      };
    };

    remote = {
      address = mkOption {
        type = types.str;
        default = "auto";
        description = "Java server address Geyser connects to. Plugin installs usually keep auto.";
      };

      port = mkOption {
        type = types.port;
        default = 25565;
        description = "Java server port Geyser connects to when address is explicit.";
      };

      authType = mkOption {
        type = types.enum [
          "online"
          "offline"
          "floodgate"
        ];
        default = "online";
        description = "Java authentication mode used by Geyser.";
      };

      useProxyProtocol = mkOption {
        type = types.bool;
        default = false;
        description = "Whether Geyser uses PROXY protocol when connecting to Java.";
      };

      forwardHostname = mkOption {
        type = types.bool;
        default = false;
        description = "Whether Geyser forwards the Bedrock hostname to the Java server.";
      };
    };

    floodgateKeyFile = mkOption {
      type = types.str;
      default = "key.pem";
      description = "Floodgate key path used when Geyser is not running beside a Floodgate plugin.";
    };

    commandSuggestions = mkOption {
      type = types.bool;
      default = true;
      description = "Whether Bedrock clients receive Java command suggestions.";
    };

    passthrough = {
      motd = mkOption {
        type = types.bool;
        default = true;
        description = "Whether Geyser relays the Java MOTD.";
      };

      playerCounts = mkOption {
        type = types.bool;
        default = true;
        description = "Whether Geyser relays Java player counts.";
      };

      legacyPing = mkOption {
        type = types.bool;
        default = false;
        description = "Whether Geyser uses legacy ping passthrough.";
      };

      interval = mkOption {
        type = types.ints.positive;
        default = 3;
        description = "Seconds between passthrough pings.";
      };
    };

    forwardPlayerPing = mkOption {
      type = types.bool;
      default = false;
      description = "Whether Geyser forwards Bedrock player ping to Java.";
    };

    maxPlayers = mkOption {
      type = types.ints.positive;
      default = 100;
      description = "Displayed maximum Bedrock player count.";
    };

    debug = mkOption {
      type = types.bool;
      default = false;
      description = "Whether Geyser logs debug messages.";
    };

    showCooldown = mkOption {
      type = types.oneOf [
        types.bool
        (types.enum [
          "title"
          "actionbar"
        ])
      ];
      default = "title";
      description = "Bedrock cooldown indicator mode.";
    };

    showCoordinates = mkOption {
      type = types.bool;
      default = true;
      description = "Whether Geyser shows coordinates to Bedrock players.";
    };

    disableBedrockScaffolding = mkOption {
      type = types.bool;
      default = false;
      description = "Whether Geyser blocks Bedrock scaffolding-style bridging.";
    };

    emoteOffhandWorkaround = mkOption {
      type = types.enum [
        "disabled"
        "no-emotes"
        "emotes-and-offhand"
      ];
      default = "disabled";
      description = "Geyser emote/offhand workaround mode.";
    };

    cacheImages = mkOption {
      type = types.ints.unsigned;
      default = 0;
      description = "Days Geyser caches downloaded images.";
    };

    allowCustomSkulls = mkOption {
      type = types.bool;
      default = true;
      description = "Whether Geyser displays custom skulls.";
    };

    maxVisibleCustomSkulls = mkOption {
      type = types.int;
      default = 128;
      description = "Maximum visible custom skull count per Bedrock player, or -1 for unlimited.";
    };

    customSkullRenderDistance = mkOption {
      type = types.ints.unsigned;
      default = 32;
      description = "Custom skull render radius in blocks.";
    };

    addNonBedrockItems = mkOption {
      type = types.bool;
      default = true;
      description = "Whether Geyser maps Java-only items and blocks for Bedrock.";
    };

    aboveBedrockNetherBuilding = mkOption {
      type = types.bool;
      default = false;
      description = "Whether Geyser works around Bedrock Nether build-height limits.";
    };

    forceResourcePacks = mkOption {
      type = types.bool;
      default = true;
      description = "Whether Bedrock players must accept resource packs.";
    };

    xboxAchievements = mkOption {
      type = types.bool;
      default = false;
      description = "Whether Xbox achievements can unlock through Geyser.";
    };

    logPlayerIpAddresses = mkOption {
      type = types.bool;
      default = true;
      description = "Whether Geyser logs Bedrock player IP addresses.";
    };

    notifyOnNewBedrockUpdate = mkOption {
      type = types.bool;
      default = true;
      description = "Whether Geyser alerts when a newer Bedrock version appears.";
    };

    unusableSpaceBlock = mkOption {
      type = types.str;
      default = "minecraft:barrier";
      description = "Java item used for unavailable Bedrock inventory slots.";
    };

    metrics.enable = mkOption {
      type = types.bool;
      default = false;
      description = "Whether Geyser bStats metrics are enabled.";
    };

    scoreboardPacketThreshold = mkOption {
      type = types.ints.positive;
      default = 20;
      description = "Scoreboard packets per second before Geyser throttles scoreboard updates.";
    };

    enableProxyConnections = mkOption {
      type = types.bool;
      default = false;
      description = "Whether Geyser permits UDP proxy connections.";
    };

    mtu = mkOption {
      type = types.ints.positive;
      default = 1400;
      description = "Bedrock network MTU.";
    };

    useDirectConnection = mkOption {
      type = types.bool;
      default = true;
      description = "Whether plugin Geyser connects Bedrock players directly into the Java server internals.";
    };

    disableCompression = mkOption {
      type = types.bool;
      default = true;
      description = "Whether Geyser disables Java compression for Bedrock players.";
    };

    settings = mkOption {
      type = types.attrsOf formatValueType;
      default = {};
      description = "Raw Geyser config.yml settings merged over the typed options.";
    };
  };

  config = mkIf cfg.enable {
    services.velocity = {
      enable = lib.mkDefault true;
      plugins.geyser = {
        src = cfg.package;
        fileName = "Geyser-Velocity.jar";
      };
      configFiles.${velocityPluginPath} = renderedConfig;
    };

    ix.networking.portClaims.geyser = {
      protocol = "udp";
      inherit (cfg.bedrock) address port;
      description = "Geyser Bedrock listener";
    };

    networking.firewall.allowedUDPPorts = lib.optional cfg.bedrock.openFirewall cfg.bedrock.port;
  };
}
