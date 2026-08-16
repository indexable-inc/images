# Clone a git repository on first boot. The clone is idempotent: subsequent
# boots see `.git` already present and do nothing.
# TODO: use cross-VM shared CAS to significantly speed up clones
{
  config,
  lib,
  pkgs,
  ...
}: let
  inherit
    (lib)
    mkEnableOption
    mkOption
    mkIf
    types
    ;
  cfg = config.services.git-clone;

  # The host to scope credentials to, taken from the URL rather than asked for
  # again: a second option could disagree with the thing being cloned, and the
  # disagreement would surface as an authentication failure rather than as a
  # config error.
  cloneHost = let
    afterScheme = lib.last (lib.splitString "://" cfg.url);
    # Drop any `user@` and everything from the first `/`, leaving host[:port].
    authority = lib.head (lib.splitString "/" afterScheme);
  in
    lib.last (lib.splitString "@" authority);
in {
  options.services.git-clone = {
    enable = mkEnableOption "clone a git repo on boot";

    url = mkOption {type = types.str;};

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

    tokenFile = mkOption {
      type = types.nullOr types.str;
      default = null;
      example = "/run/secrets/github/token";
      description = ''
        Runtime path to a file holding a token that authenticates the clone,
        for a private repository. Typically where a `deployment.secrets` entry
        materializes; the path is read at request time, so it need not exist
        at build time or at boot.

        Setting this turns on {option}`programs.git-token-auth` for the host
        in {option}`url`, so the same token also answers the fetches and
        pushes made from that checkout later, and adds a preflight that
        fails the clone with a message naming the missing secret rather than
        letting a private repository answer 404.
      '';
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
    environment.systemPackages = [pkgs.gitoxide];

    programs = {
      git = {
        # A VM holding a checkout wants git, and the config below needs
        # somewhere to land. mkDefault so a consumer can still turn it off.
        enable = lib.mkDefault true;

        # The clone runs as root and whoever shells in later is not
        # necessarily root; without this git refuses the tree as dubiously
        # owned, which reads as a repository problem rather than an ownership
        # one.
        config.safe.directory = cfg.dest;
      };

      # Authenticating the clone and authenticating the checkout afterwards are
      # the same credential, so this sets the general option rather than
      # carrying a private copy of the wiring. `hosts` is a list, so a consumer
      # naming a second forge adds to this rather than replacing it.
      git-token-auth = mkIf (cfg.tokenFile != null) {
        inherit (cfg) tokenFile;
        hosts = [cloneHost];
      };
    };

    systemd.services.git-clone = {
      description = "Clone ${cfg.url}";
      after = ["network-online.target"];
      wants = ["network-online.target"];
      wantedBy = lib.optional (cfg.activation == "multi-user") "multi-user.target";
      serviceConfig = {
        Type = "oneshot";
        RemainAfterExit = true;
        # Refuse before reaching the network when the token cannot produce a
        # credential. Without it an absent token clones anonymously and a
        # private repository answers 404, which reads as a wrong URL and
        # points nowhere near the real cause.
        ExecStartPre = lib.optional (cfg.tokenFile != null) config.programs.git-token-auth.preflight;
      };
      path = [
        pkgs.coreutils
        pkgs.gitoxide
      ];
      script = let
        depthFlag = lib.optionalString cfg.shallow "--depth 1";
        refFlag = lib.optionalString (cfg.ref != null) "--ref ${lib.escapeShellArg cfg.ref}";
        destParent = builtins.dirOf cfg.dest;
      in ''
        if [ ! -d "${cfg.dest}/.git" ]; then
          mkdir -p ${lib.escapeShellArg destParent}
          gix clone ${depthFlag} ${refFlag} ${lib.escapeShellArg cfg.url} ${lib.escapeShellArg cfg.dest}
        fi
      '';
    };

    systemd.timers.git-clone = mkIf (cfg.activation == "timer") {
      description = "Start git clone after boot";
      wantedBy = ["timers.target"];
      timerConfig = {
        OnBootSec = "15s";
        Unit = "git-clone.service";
      };
    };
  };
}
