# Floodgate auth bridge, installed as a Velocity plugin beside Geyser.
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

  cfg = config.services.floodgate;
  formatValueType = (pkgs.formats.json { }).type;
  velocityConfigPath = "plugins/floodgate/config.yml";
  velocityProxyConfigPath = "plugins/floodgate/proxy-config.yml";

  metricsConfig = {
    enabled = cfg.metrics.enable;
  }
  // lib.optionalAttrs (cfg.metrics.uuid != null) {
    inherit (cfg.metrics) uuid;
  };

  playerLinkConfig = {
    inherit (cfg.playerLink) allowed type;
    enabled = cfg.playerLink.enable;
    "require-link" = cfg.playerLink.requireLink;
    "enable-own-linking" = cfg.playerLink.enableOwnLinking;
    "link-code-timeout" = cfg.playerLink.linkCodeTimeout;
    "enable-global-linking" = cfg.playerLink.enableGlobalLinking;
  };

  renderedConfig = {
    "key-file-name" = cfg.keyFileName;
    "username-prefix" = cfg.usernamePrefix;
    "replace-spaces" = cfg.replaceSpaces;
    disconnect = {
      "invalid-key" = cfg.disconnect.invalidKey;
      "invalid-arguments-length" = cfg.disconnect.invalidArgumentsLength;
    };
    "player-link" = playerLinkConfig;
    metrics = metricsConfig;
    "config-version" = 3;
  }
  // cfg.settings;

  renderedProxyConfig = {
    "send-floodgate-data" = cfg.sendFloodgateData;
  }
  // cfg.proxySettings;
in
{
  options.services.floodgate = {
    enable = mkEnableOption "Floodgate Bedrock identity bridge";

    platform = mkOption {
      type = types.enum [ "velocity" ];
      default = "velocity";
      description = "Platform integration used for Floodgate.";
    };

    package = mkOption {
      type = types.package;
      default = ix.artifacts.minecraft.velocityPluginCatalog.floodgate-velocity.src;
      defaultText = lib.literalExpression "ix.artifacts.minecraft.velocityPluginCatalog.floodgate-velocity.src";
      description = "Floodgate Velocity plugin jar.";
    };

    keyFileName = mkOption {
      type = types.str;
      default = "key.pem";
      description = "Floodgate private key file name under its plugin data directory.";
    };

    usernamePrefix = mkOption {
      type = types.str;
      default = ".";
      description = "Prefix added to Bedrock player names.";
    };

    replaceSpaces = mkOption {
      type = types.bool;
      default = true;
      description = "Whether spaces in Bedrock player names are replaced with underscores.";
    };

    sendFloodgateData = mkOption {
      type = types.bool;
      default = false;
      description = "Whether proxy Floodgate sends encrypted Bedrock player data to backend servers with Floodgate installed.";
    };

    disconnect = {
      invalidKey = mkOption {
        type = types.str;
        default = "Please connect through the official Geyser";
        description = "Disconnect message for Geyser users with an invalid Floodgate key.";
      };

      invalidArgumentsLength = mkOption {
        type = types.str;
        default = "Expected {} arguments, got {}. Is Geyser up-to-date?";
        description = "Disconnect message for Geyser users with invalid Floodgate data.";
      };
    };

    playerLink = {
      enable = mkOption {
        type = types.bool;
        default = true;
        description = "Whether Floodgate account linking is enabled.";
      };

      requireLink = mkOption {
        type = types.bool;
        default = false;
        description = "Whether Bedrock players must link a Java account before joining.";
      };

      enableOwnLinking = mkOption {
        type = types.bool;
        default = false;
        description = "Whether Floodgate uses a local linking database implementation.";
      };

      allowed = mkOption {
        type = types.bool;
        default = true;
        description = "Whether players may use link and unlink commands.";
      };

      linkCodeTimeout = mkOption {
        type = types.ints.positive;
        default = 300;
        description = "Seconds before a Floodgate link code expires.";
      };

      type = mkOption {
        type = types.str;
        default = "sqlite";
        description = "Local player-link database type.";
      };

      enableGlobalLinking = mkOption {
        type = types.bool;
        default = true;
        description = "Whether Floodgate global account linking is enabled.";
      };
    };

    metrics = {
      enable = mkOption {
        type = types.bool;
        default = false;
        description = "Whether Floodgate bStats metrics are enabled.";
      };

      uuid = mkOption {
        type = types.nullOr types.str;
        default = null;
        description = "Floodgate bStats UUID. Leave null when metrics are disabled.";
      };
    };

    settings = mkOption {
      type = types.attrsOf formatValueType;
      default = { };
      description = "Raw Floodgate config.yml settings merged over the typed options.";
    };

    proxySettings = mkOption {
      type = types.attrsOf formatValueType;
      default = { };
      description = "Raw Floodgate proxy-config.yml settings merged over the typed options.";
    };
  };

  config = mkIf cfg.enable {
    services.velocity = {
      enable = lib.mkDefault true;
      plugins.floodgate = {
        src = cfg.package;
        fileName = "floodgate-velocity.jar";
      };
      configFiles = {
        ${velocityConfigPath} = renderedConfig;
        ${velocityProxyConfigPath} = renderedProxyConfig;
      };
    };

    services.geyser.remote.authType = lib.mkIf config.services.geyser.enable (
      lib.mkDefault "floodgate"
    );
  };
}
