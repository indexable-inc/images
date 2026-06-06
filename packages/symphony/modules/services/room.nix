{
  config,
  lib,
  ...
}:
let
  inherit (lib)
    mkEnableOption
    mkIf
    mkOption
    types
    ;

  cfg = config.services.room;
in
{
  options.services.room = {
    enable = mkEnableOption "multiplayer team room";

    package = mkOption {
      type = types.package;
      description = "Package that provides the room server.";
    };

    host = mkOption {
      type = types.str;
      default = "0.0.0.0";
      description = "Address the room server binds.";
    };

    port = mkOption {
      type = types.port;
      default = 8080;
      description = "TCP port served by the room server.";
    };

    stateDir = mkOption {
      type = types.path;
      default = "/var/lib/room";
      description = "Directory where room-server stores its SQLite database.";
    };

    sitePackage = mkOption {
      type = types.nullOr types.package;
      default = null;
      description = "Optional package containing the built Room web UI.";
    };

    wtHost = mkOption {
      type = types.str;
      default = "127.0.0.1";
      description = "Host advertised to browser WebTransport clients.";
    };

    wtPort = mkOption {
      type = types.port;
      default = 4433;
      description = "UDP port served by the WebTransport listener.";
    };

    openFirewall = mkOption {
      type = types.bool;
      default = true;
      description = "Whether to open the room port in the in-guest firewall.";
    };
  };

  config = mkIf cfg.enable {
    systemd.services.room = {
      description = "Multiplayer team room";
      wantedBy = [ "multi-user.target" ];
      serviceConfig = {
        Type = "simple";
        DynamicUser = true;
        ExecStart = lib.escapeShellArgs [
          (lib.getExe cfg.package)
          "--host"
          cfg.host
          "--port"
          (toString cfg.port)
          "--state-dir"
          cfg.stateDir
          "--wt-host"
          cfg.wtHost
          "--wt-port"
          (toString cfg.wtPort)
        ];
        Environment = lib.optional (cfg.sitePackage != null) "ROOM_SITE_DIR=${cfg.sitePackage}";
        Restart = "on-failure";
        StateDirectory = "room";
        NoNewPrivileges = true;
        PrivateTmp = true;
        ProtectHome = true;
        ProtectSystem = "strict";
        ReadWritePaths = [ cfg.stateDir ];
      };
    };

    networking.firewall.allowedTCPPorts = lib.optional cfg.openFirewall cfg.port;
    networking.firewall.allowedUDPPorts = lib.optional cfg.openFirewall cfg.wtPort;
  };
}
