# Browser-accessible remote desktop backed by Xpra's built-in HTML5 client.
{
  config,
  ix,
  lib,
  pkgs,
  ...
}:
let
  inherit (lib)
    mkDefault
    mkEnableOption
    mkIf
    mkOption
    mkPackageOption
    types
    ;
  cfg = config.services.remote-desktop;

  defaultSession = ix.writeNushellApplication pkgs {
    name = "ix-remote-desktop-session";
    runtimeInputs = [
      pkgs.icewm
      pkgs.xterm
    ];
    text = ''
      def main [] {
        job spawn { ^xterm }
        exec icewm-session
      }
    '';
  };

  flagValueType = types.oneOf [
    types.bool
    types.int
    types.str
    (types.listOf types.str)
  ];

  flags = lib.cli.toCommandLineGNU { } cfg.settings;

  launcher = ix.writeNushellApplication pkgs {
    name = "ix-remote-desktop";
    runtimeInputs = [
      cfg.package
    ];
    text = ''
      def main [] {
        let args = ${
          builtins.toJSON (
            [
              "start-desktop"
              cfg.display
            ]
            ++ flags
          )
        }
        exec xpra ...$args
      }
    '';
  };
in
{
  options.services.remote-desktop = {
    enable = mkEnableOption "browser-accessible Xpra remote desktop";

    package = mkPackageOption pkgs "xpra" { };

    port = mkOption {
      type = types.port;
      default = 6080;
      description = "TCP port for the Xpra HTML5 client.";
    };

    bindAddress = mkOption {
      type = types.str;
      default = "0.0.0.0";
      description = "Address Xpra binds for browser clients.";
    };

    display = mkOption {
      type = types.str;
      default = ":100";
      description = "X display number managed by Xpra.";
    };

    resolution = mkOption {
      type = types.str;
      default = "1920x1080";
      description = "Initial virtual display resolution.";
    };

    desktopCommand = mkOption {
      type = types.str;
      default = lib.getExe defaultSession;
      defaultText = lib.literalExpression ''"${lib.getExe defaultSession}"'';
      description = "Command Xpra starts as the desktop session.";
    };

    auth = mkOption {
      type = types.str;
      default = "none";
      description = "Xpra authentication module for incoming clients.";
    };

    settings = mkOption {
      type = types.attrsOf flagValueType;
      default = { };
      description = ''
        Flags passed to `xpra start-desktop` rendered via `lib.cli.toCommandLineGNU`.
        Each entry becomes `--key=value`; `true` becomes a bare `--key`, `false`
        omits the flag, and list values render as repeated `--key=elem` entries.
        Convenience options (`port`, `bindAddress`, `display`, `resolution`,
        `desktopCommand`, `auth`) seed this set via `mkDefault`, so a direct
        assignment here wins.
      '';
    };
  };

  config = mkIf cfg.enable {
    services.remote-desktop.settings = {
      start = mkDefault cfg.desktopCommand;
      bind-tcp = mkDefault "${cfg.bindAddress}:${toString cfg.port}";
      auth = mkDefault cfg.auth;
      resize-display = mkDefault cfg.resolution;
      socket-dirs = mkDefault "/run/remote-desktop";
      html = mkDefault "on";
      ssl = mkDefault "off";
      daemon = mkDefault "no";
      mdns = mkDefault "no";
      pulseaudio = mkDefault "no";
      notifications = mkDefault "no";
      webcam = mkDefault "no";
      printing = mkDefault "no";
      file-transfer = mkDefault "off";
      open-files = mkDefault "off";
      clipboard = mkDefault "on";
    };

    environment.systemPackages = [
      cfg.package
      pkgs.icewm
      pkgs.xterm
    ];

    networking.firewall.allowedTCPPorts = [ cfg.port ];

    users.groups.remote-desktop = { };
    users.users.remote-desktop = {
      description = "Remote desktop service user";
      isSystemUser = true;
      group = "remote-desktop";
      home = "/var/lib/remote-desktop";
    };

    systemd.services.remote-desktop = {
      description = "Xpra remote desktop";
      after = [ "network-online.target" ];
      wants = [ "network-online.target" ];
      wantedBy = [ "multi-user.target" ];
      environment.HOME = "/var/lib/remote-desktop";
      serviceConfig = {
        Type = "simple";
        User = "remote-desktop";
        Group = "remote-desktop";
        StateDirectory = "remote-desktop";
        RuntimeDirectory = "remote-desktop";
        WorkingDirectory = "/var/lib/remote-desktop";
        ExecStart = lib.getExe launcher;
        Restart = "on-failure";
      };
    };
  };
}
