{
  config,
  lib,
  ...
}: let
  cfg = config.users.andrewgazelka;
in {
  options.users.andrewgazelka = {
    blockedHosts = lib.mkOption {
      type = lib.types.listOf lib.types.str;
      default = [];
      description = "Domains whose browser cookies are cleared on activation.";
    };

    browserBlockedHosts = lib.mkOption {
      type = lib.types.listOf lib.types.str;
      default = [];
      description = "Domains redirected by the workstation browser-tab guard.";
    };

    configurationRevision = lib.mkOption {
      type = lib.types.nullOr lib.types.str;
      default = null;
      description = "Revision of the consuming host configuration for provenance.";
    };

    packages = {
      ix = lib.mkOption {
        type = lib.types.nullOr lib.types.package;
        default = null;
        description = "Host-native ix CLI supplied by the consuming flake.";
      };
      lifelog = lib.mkOption {
        type = lib.types.nullOr lib.types.package;
        default = null;
        description = "Host-native lifelog package supplied by the consuming flake.";
      };
      mercuryCli = lib.mkOption {
        type = lib.types.nullOr lib.types.package;
        default = null;
        description = "Host-native Mercury CLI supplied by the consuming flake.";
      };
      typenix = lib.mkOption {
        type = lib.types.nullOr lib.types.package;
        default = null;
        description = "Host-native typenix package supplied by the consuming flake.";
      };
    };

    paths = {
      indexCheckout = lib.mkOption {
        type = lib.types.str;
        default = "${config.home.homeDirectory}/Projects/indexable-inc/index";
        defaultText = lib.literalExpression ''"''${config.home.homeDirectory}/Projects/indexable-inc/index"'';
        description = "Mutable index checkout used by workstation-only development services.";
      };
      ixCheckout = lib.mkOption {
        type = lib.types.str;
        default = "${config.home.homeDirectory}/Projects/indexable-inc/ix";
        defaultText = lib.literalExpression ''"''${config.home.homeDirectory}/Projects/indexable-inc/ix"'';
        description = "Mutable ix checkout used by workstation development tools.";
      };
      privateConfigDirectory = lib.mkOption {
        type = lib.types.str;
        default = "${config.home.homeDirectory}/.config/nix";
        defaultText = lib.literalExpression ''"''${config.home.homeDirectory}/.config/nix"'';
        description = "Private runtime configuration directory containing secret files.";
      };
      symphonyPack = lib.mkOption {
        type = lib.types.str;
        default = "${cfg.paths.indexCheckout}/packages/agent/symphony/workflows/indexable";
        defaultText = lib.literalExpression ''"''${config.users.andrewgazelka.paths.indexCheckout}/packages/agent/symphony/workflows/indexable"'';
        description = "Mutable Symphony workflow pack directory.";
      };
      vscodeIslands = lib.mkOption {
        type = lib.types.nullOr lib.types.path;
        default = null;
        description = "Source tree for the VS Code Islands extension.";
      };
    };

    pinTimestamps = {
      index = lib.mkOption {
        type = lib.types.ints.unsigned;
        default = 0;
        description = "Index flake commit epoch displayed by Starship.";
      };
      ix = lib.mkOption {
        type = lib.types.ints.unsigned;
        default = 0;
        description = "ix flake commit epoch displayed by Starship.";
      };
    };

    sshSigningPublicKey = lib.mkOption {
      type = lib.types.str;
      default = builtins.head (
        builtins.filter (line: lib.hasInfix "signing" (lib.toLower line)) (
          lib.splitString "\n" (builtins.readFile ./config/ssh-keys/andrewgazelka.pub)
        )
      );
      defaultText = lib.literalExpression "the public key labelled signing in users/andrewgazelka/config/ssh-keys/andrewgazelka.pub";
      description = "Public SSH signing key line; never a private key or credential.";
    };

    ssh = {
      matchBlocks = lib.mkOption {
        type = lib.types.attrsOf (lib.types.attrsOf lib.types.anything);
        default = {};
        description = "Private, host-specific SSH match blocks supplied by the consumer.";
      };
      knownHosts = lib.mkOption {
        type = lib.types.nullOr lib.types.lines;
        default = null;
        description = "Private fleet known-host inventory supplied by the consumer.";
      };
    };

    rbw = {
      email = lib.mkOption {
        type = lib.types.nullOr lib.types.str;
        default = null;
        description = "Private Vaultwarden account name supplied by the consumer.";
      };
      baseUrl = lib.mkOption {
        type = lib.types.nullOr lib.types.str;
        default = null;
        description = "Private Vaultwarden endpoint supplied by the consumer.";
      };
    };
  };
}
