# Clone a git repository on first boot. The clone is idempotent: subsequent
# boots see `.git` already present and do nothing.
# TODO: use cross-VM shared CAS to significantly speed up clones
{
  config,
  lib,
  pkgs,
  ...
}:
let
  inherit (lib)
    mkEnableOption
    mkOption
    mkIf
    mkMerge
    types
    ;
  cfg = config.services.git-clone;
in
{
  options.services.git-clone = {
    enable = mkEnableOption "clone a git repo on boot";

    url = mkOption { type = types.str; };

    dest = mkOption {
      type = types.str;
      default = "/repo";
    };

    shallow = mkOption {
      type = types.bool;
      default = true;
    };

    ref = mkOption {
      type = types.nullOr types.str;
      default = null;
    };

    activation = mkOption {
      type = types.enum [
        "multi-user"
        "timer"
      ];
      default = "multi-user";
      description = ''
        How the clone is started. Use timer for large repositories that should
        be fetched after boot readiness instead of blocking multi-user.target.
      '';
    };
  };

  config = mkIf cfg.enable {
    environment.systemPackages = [ pkgs.gitoxide ];

    systemd = mkMerge [
      {
        services.git-clone = {
          description = "Clone ${cfg.url}";
          after = [ "network-online.target" ];
          wants = [ "network-online.target" ];
          wantedBy = lib.optional (cfg.activation == "multi-user") "multi-user.target";
          serviceConfig = {
            Type = "oneshot";
            RemainAfterExit = true;
          };
          path = [
            pkgs.coreutils
            pkgs.gitoxide
          ];
          script =
            let
              depthFlag = lib.optionalString cfg.shallow "--depth 1";
              refFlag = lib.optionalString (cfg.ref != null) "--ref ${lib.escapeShellArg cfg.ref}";
              destParent = builtins.dirOf cfg.dest;
            in
            ''
              if [ ! -d "${cfg.dest}/.git" ]; then
                mkdir -p ${lib.escapeShellArg destParent}
                gix clone ${depthFlag} ${refFlag} ${lib.escapeShellArg cfg.url} ${lib.escapeShellArg cfg.dest}
              fi
            '';
        };
      }
      (mkIf (cfg.activation == "timer") {
        timers.git-clone = {
          description = "Start git clone after boot";
          wantedBy = [ "timers.target" ];
          timerConfig = {
            OnBootSec = "15s";
            Unit = "git-clone.service";
          };
        };
      })
    ];
  };
}
