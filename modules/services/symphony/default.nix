# NixOS service module for the Symphony runtime.
#
# Minimal opinionated systemd unit. Reads secrets from an EnvironmentFile
# you control, so you can wire any secret manager (sops-nix, agenix,
# Bitwarden Secrets Manager, AWS Secrets Manager, etc.) underneath. For
# Bitwarden Secrets Manager specifically, set `secretsCommand` to a
# `bws run -- ...` invocation; the unit will wrap ExecStart with it.
{
  config,
  lib,
  pkgs,
  ...
}: let
  inherit
    (lib)
    mkEnableOption
    mkIf
    mkOption
    optionalString
    types
    ;

  cfg = config.services.symphony;
in {
  options.services.symphony = {
    enable = mkEnableOption "Symphony runtime";

    package = mkOption {
      type = types.package;
      description = "Symphony package (provides /bin/symphony from this flake's default output).";
    };

    user = mkOption {
      type = types.str;
      default = "symphony";
      description = "Unix user the service runs as. Set to an existing user, or let DynamicUser handle it.";
    };

    stateDir = mkOption {
      type = types.path;
      default = "/var/lib/symphony";
      description = "Directory for runs, workspaces, logs, and the staged runtime copy.";
    };

    httpPort = mkOption {
      type = types.port;
      default = 4040;
      description = "Phoenix HTTP listener port.";
    };

    primaryRepo = mkOption {
      type = types.nullOr types.path;
      default = null;
      description = "Absolute path to the primary repository checkout (SYMPHONY_PRIMARY_REPO).";
    };

    repoRoot = mkOption {
      type = types.nullOr types.path;
      default = null;
      description = "Optional parent directory of sibling repository checkouts (SYMPHONY_REPO_ROOT). Defaults to the parent of primaryRepo.";
    };

    workflowPack = mkOption {
      type = types.str;
      default = "example";
      description = "Built-in workflow pack name; ignored when packDir is set.";
    };

    packDir = mkOption {
      type = types.nullOr types.path;
      default = null;
      description = "Absolute path to an external workflow pack (SYMPHONY_PACK_DIR). Takes precedence over workflowPack.";
    };

    roomRegistryUrl = mkOption {
      type = types.nullOr types.str;
      default = null;
      description = ''
        Central room.ix.dev base URL each run's room-server registers its
        backend with (SYMPHONY_ROOM_REGISTRY_URL). Drives both the room UI's
        transcript view and the Slack "Run details" deep link. Unset disables
        registration and the Slack link. The matching write token is a secret;
        supply SYMPHONY_ROOM_REGISTRY_TOKEN via environmentFile.
      '';
    };

    roomAdvertiseHost = mkOption {
      type = types.nullOr types.str;
      default = null;
      description = ''
        Address a provisioned per-run room-server binds and advertises so
        room.ix.dev can reach it to proxy the run's transcript
        (SYMPHONY_ROOM_ADVERTISE_HOST). Set to this host's tailnet address when
        room.ix.dev runs elsewhere; unset keeps the loopback default, reachable
        only when room.ix.dev shares the host.
      '';
    };

    roomServerUrl = mkOption {
      type = types.nullOr types.str;
      default = null;
      description = ''
        Standing room-server URL for `:local` / `{:room, url}` placements that
        do not provision their own per-run server (SYMPHONY_ROOM_SERVER_URL).
      '';
    };

    extraEnvironment = mkOption {
      type = types.attrsOf types.str;
      default = {};
      description = ''
        Additional environment variables exported to the service. Use for
        non-secret config: LINEAR_WORKSPACE_SLUG, SYMPHONY_BOT_USERNAME,
        SYMPHONY_BOT_EMAIL, SYMPHONY_GITHUB_APP_OWNER_REPO,
        SYMPHONY_GITHUB_STATS_QUERY, SYMPHONY_SLACK_NOTIFY_CHANNEL, etc.
      '';
    };

    environmentFile = mkOption {
      type = types.nullOr types.path;
      default = null;
      description = ''
        Path to a systemd EnvironmentFile holding secrets:
        LINEAR_API_KEY, GITHUB_TOKEN, LINEAR_WEBHOOK_SECRET,
        GITHUB_WEBHOOK_SECRET, SLACK_SIGNING_SECRET, SLACK_BOT_OAUTH_TOKEN,
        SYMPHONY_GITHUB_APP_PRIVATE_KEY_BASE64, SYMPHONY_ROOM_REGISTRY_TOKEN,
        etc.
        Wire this to whichever secret manager you use (sops-nix, agenix, ...).
        Leave null if you use secretsCommand instead.
      '';
    };

    secretsCommand = mkOption {
      type = types.nullOr (types.listOf types.str);
      default = null;
      example = [
        "bws"
        "run"
        "--project-id"
        "symphony-prod"
        "--"
      ];
      description = ''
        Optional command that wraps ExecStart and injects secrets into the
        environment. Designed for Bitwarden Secrets Manager (`bws run --
        ...`) or any compatible secret-injecting CLI. The wrapper command
        must exec its trailing arguments. Place the bws binary on the
        service's PATH via `path = [ pkgs.bws ];` or by adding it to
        runtimeInputs of the symphony package.

        When set, the unit also expects BWS_ACCESS_TOKEN (or equivalent)
        to be exported via environmentFile or extraEnvironment.
      '';
    };

    path = mkOption {
      type = types.listOf types.package;
      default = [];
      description = "Extra packages on the service PATH (e.g. pkgs.bws when using secretsCommand).";
    };

    hostRuntime = mkOption {
      default = {};
      description = ''
        The host codex placement. When enabled, a workflow node that
        declares `location: host` (or the run's resolved fallback) runs
        codex directly on this machine as a real OS user, with no VM. The
        per-run room-server and the codex process it spawns run as
        `user` inside that user's home directory, launched as transient
        `systemd-run --uid` units. This option wires the polkit grant,
        PATH, and environment that path needs. It stays inert until
        `enable` is set.
      '';
      type = types.submodule {
        options = {
          enable = mkEnableOption "the host codex placement";

          user = mkOption {
            type = types.str;
            default = "";
            description = "OS user codex runs as for host placement (SYMPHONY_HOST_USER). Must already exist with a home directory.";
          };

          group = mkOption {
            type = types.nullOr types.str;
            default = null;
            description = "OS group for host runs (SYMPHONY_HOST_GROUP); omitted uses the user's primary group.";
          };

          workspacesDir = mkOption {
            type = types.nullOr types.path;
            default = null;
            description = "Parent directory for run checkouts (SYMPHONY_HOST_WORKSPACES_DIR); defaults to <user home>/symphony-workspaces.";
          };

          roomServerPackage = mkOption {
            type = types.nullOr types.package;
            default = null;
            description = "Package providing the codex-wrapped room-server launched as the host user (this flake's room-server output). Used by the per-run host placement.";
          };

          keep = mkOption {
            type = types.bool;
            default = false;
            description = "Leave the unit and checkout in place after the turn for inspection (SYMPHONY_HOST_KEEP).";
          };
        };
      };
    };
  };

  config = mkIf cfg.enable {
    assertions = [
      {
        assertion = !cfg.hostRuntime.enable || cfg.hostRuntime.user != "";
        message = "services.symphony.hostRuntime.user must be set when hostRuntime.enable is true.";
      }
      {
        assertion = !cfg.hostRuntime.enable || cfg.hostRuntime.roomServerPackage != null;
        message = "services.symphony.hostRuntime.roomServerPackage must be set when hostRuntime.enable is true.";
      }
    ];

    # The host runtime calls systemd's StartTransientUnit over D-Bus to run
    # codex as another user. A non-root service needs polkit authorization
    # for that. Scope the grant to the "symphony-host-" unit-name prefix so
    # the service cannot manage unrelated system units. See systemd-run(1)
    # and the polkit systemd1 actions documented at
    # https://www.freedesktop.org/software/systemd/man/latest/org.freedesktop.systemd1.html
    security.polkit = lib.mkIf cfg.hostRuntime.enable {
      enable = true;
      extraConfig = ''
        polkit.addRule(function(action, subject) {
          if (subject.user == "${cfg.user}" &&
              action.id == "org.freedesktop.systemd1.manage-units") {
            var unit = action.lookup("unit");
            if (unit && unit.indexOf("symphony-host-") == 0) {
              return polkit.Result.YES;
            }
          }
        });
      '';
    };

    users.users = lib.mkIf (cfg.user == "symphony") {
      symphony = {
        isSystemUser = true;
        group = "symphony";
        home = cfg.stateDir;
      };
    };

    users.groups = lib.mkIf (cfg.user == "symphony") {
      symphony = {};
    };

    systemd.tmpfiles.rules = [
      "d ${cfg.stateDir} 0750 ${cfg.user} ${cfg.user} -"
      "d ${cfg.stateDir}/workspaces 0750 ${cfg.user} ${cfg.user} -"
      "d ${cfg.stateDir}/runs 0750 ${cfg.user} ${cfg.user} -"
      "d ${cfg.stateDir}/log 0750 ${cfg.user} ${cfg.user} -"
    ];

    systemd.services.symphony = {
      description = "Symphony runtime";
      wantedBy = ["multi-user.target"];
      after = ["network-online.target"];
      wants = ["network-online.target"];

      path =
        cfg.path
        ++ lib.optionals cfg.hostRuntime.enable [
          pkgs.systemd
          pkgs.getent
          cfg.hostRuntime.roomServerPackage
        ];

      environment =
        {
          SYMPHONY_STATE_DIR = cfg.stateDir;
          SYMPHONY_WORKSPACES_DIR = "${cfg.stateDir}/workspaces";
          SYMPHONY_RUNS_DIR = "${cfg.stateDir}/runs";
          SYMPHONY_LOGS_ROOT = "${cfg.stateDir}/log";
          SYMPHONY_HTTP_PORT = toString cfg.httpPort;
          SYMPHONY_WORKFLOW_PACK = cfg.workflowPack;
        }
        // (lib.optionalAttrs (cfg.primaryRepo != null) {
          SYMPHONY_PRIMARY_REPO = toString cfg.primaryRepo;
        })
        // (lib.optionalAttrs (cfg.repoRoot != null) {
          SYMPHONY_REPO_ROOT = toString cfg.repoRoot;
        })
        // (lib.optionalAttrs (cfg.packDir != null) {
          SYMPHONY_PACK_DIR = toString cfg.packDir;
        })
        // (lib.optionalAttrs (cfg.roomRegistryUrl != null) {
          SYMPHONY_ROOM_REGISTRY_URL = cfg.roomRegistryUrl;
        })
        // (lib.optionalAttrs (cfg.roomAdvertiseHost != null) {
          SYMPHONY_ROOM_ADVERTISE_HOST = cfg.roomAdvertiseHost;
        })
        // (lib.optionalAttrs (cfg.roomServerUrl != null) {
          SYMPHONY_ROOM_SERVER_URL = cfg.roomServerUrl;
        })
        // (lib.optionalAttrs cfg.hostRuntime.enable (
          {
            SYMPHONY_HOST_USER = cfg.hostRuntime.user;
            SYMPHONY_HOST_ROOM_SERVER_COMMAND = lib.getExe cfg.hostRuntime.roomServerPackage;
          }
          // (lib.optionalAttrs (cfg.hostRuntime.group != null) {
            SYMPHONY_HOST_GROUP = cfg.hostRuntime.group;
          })
          // (lib.optionalAttrs (cfg.hostRuntime.workspacesDir != null) {
            SYMPHONY_HOST_WORKSPACES_DIR = toString cfg.hostRuntime.workspacesDir;
          })
          // (lib.optionalAttrs cfg.hostRuntime.keep {
            SYMPHONY_HOST_KEEP = "true";
          })
        ))
        // cfg.extraEnvironment;

      serviceConfig =
        {
          Type = "simple";
          User = cfg.user;
          Group = cfg.user;
          ExecStart = let
            symphonyBin = "${cfg.package}/bin/symphony";
            wrapper = optionalString (cfg.secretsCommand != null) (
              lib.escapeShellArgs cfg.secretsCommand + " "
            );
          in "${wrapper}${symphonyBin}";
          Restart = "on-failure";
          RestartSec = "10s";
          StateDirectory = lib.mkIf (lib.hasPrefix "/var/lib/" cfg.stateDir) (
            lib.removePrefix "/var/lib/" cfg.stateDir
          );
          # Symphony spawns codex subprocesses and clones git repos, so
          # most sandboxing options need to stay permissive. Only enable
          # the cheap, safe ones.
          NoNewPrivileges = true;
          PrivateTmp = true;
          ProtectKernelTunables = true;
          ProtectKernelModules = true;
          ProtectControlGroups = true;
        }
        // (lib.optionalAttrs (cfg.environmentFile != null) {
          EnvironmentFile = cfg.environmentFile;
        });
    };
  };
}
