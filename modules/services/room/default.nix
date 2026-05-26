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

  cfg = config.services.room;
in
{
  options.services.room = {
    enable = mkEnableOption "multiplayer team room";

    package = mkOption {
      type = types.package;
      default = (ix.packageSetFor pkgs).room;
      defaultText = lib.literalExpression "(ix.packageSetFor pkgs).room";
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

    openFirewall = mkOption {
      type = types.bool;
      default = true;
      description = "Whether to open the room port in the in-guest firewall.";
    };
  };

  config = mkIf cfg.enable {
    ix.networking.portClaims.room = {
      protocol = "tcp";
      inherit (cfg) port;
      address = cfg.host;
      description = "multiplayer team room";
    };

    systemd.services.room = {
      description = "Multiplayer team room";
      wantedBy = [ "multi-user.target" ];
      serviceConfig = ix.systemdHardening // {
        Type = "simple";
        DynamicUser = true;
        ExecStart = lib.escapeShellArgs [
          (lib.getExe cfg.package)
          "--host"
          cfg.host
          "--port"
          (toString cfg.port)
        ];
        Restart = "on-failure";
      };
    };

    networking.firewall.allowedTCPPorts = lib.optionals cfg.openFirewall [ cfg.port ];
  };
}
