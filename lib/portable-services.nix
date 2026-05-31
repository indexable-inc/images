# Portable user services: one declarative spec that renders to a native
# launchd agent on macOS and native systemd user units on Linux.
#
# Design (synthesis of the three existing approaches):
#
#   * home-manager dual-schema: produce *native* init units on each OS
#     (a real launchd plist, a real systemd unit) rather than a foreign
#     supervisor. We keep that, but the caller writes one spec instead of
#     two hand-synced schemas.
#   * services-flake: one definition, cross-platform. We keep "write once",
#     but skip the process-compose supervisor: native units integrate with
#     the OS (boot/login, logging, restart) without a long-lived parent.
#   * RFC-163 modular services: a portable option core with typed,
#     feature-tested escape hatches for the bits that do not generalize.
#     We keep that shape: `launchd.config` and `systemd.service` accept raw
#     keys merged last, so no power is lost when the portable subset is not
#     enough.
#
# The transforms (`toLaunchdConfig`, `toSystemdUnits`) are pure functions of
# a fully-defaulted spec so they can be golden-tested without evaluating a
# whole home-manager configuration. The home-manager module is a thin
# platform-dispatching wrapper around them.
{ lib }:
let
  inherit (lib)
    types
    mkOption
    optionalAttrs
    ;

  /**
    Submodule type for a single portable service.

    The option set is deliberately the *portable subset*: every field maps
    onto both launchd and systemd. Platform-specific keys go through the
    `launchd.config` / `systemd.service` escape hatches, which are merged
    over the generated unit and therefore always win.
  */
  serviceSubmodule = types.submodule (
    { name, ... }:
    {
      options = {
        enable = mkOption {
          type = types.bool;
          default = true;
          description = "Whether to generate and activate this service.";
        };

        description = mkOption {
          type = types.str;
          default = name;
          description = "Human-readable description. Defaults to the attribute name.";
        };

        command = mkOption {
          type = types.nonEmptyListOf types.str;
          example = [
            "/run/current-system/sw/bin/my-daemon"
            "--flag"
          ];
          description = ''
            Argument vector. `command` element 0 is the executable.

            On systemd the executable must be an absolute path (systemd does
            not search `PATH` for `ExecStart`). launchd has the same
            requirement in practice, so always pass an absolute path.
          '';
        };

        environment = mkOption {
          type = types.attrsOf types.str;
          default = { };
          description = "Environment variables for the process.";
        };

        workingDirectory = mkOption {
          type = types.nullOr types.str;
          default = null;
          description = "Working directory. Null leaves it unset (the init system's default).";
        };

        runAtLoad = mkOption {
          type = types.bool;
          default = true;
          description = ''
            Start the service when the user session loads (login on macOS,
            `default.target` on Linux). Ignored for interval services, which
            are driven by their schedule.
          '';
        };

        restart = mkOption {
          type = types.enum [
            "no"
            "on-failure"
            "always"
          ];
          default = "no";
          description = ''
            Restart policy.

            * `no`: run once; do not restart.
            * `on-failure`: restart only on a non-zero exit
              (launchd `KeepAlive.SuccessfulExit = false`, systemd
              `Restart = on-failure`).
            * `always`: keep the process running
              (launchd `KeepAlive = true`, systemd `Restart = always`).
          '';
        };

        interval = mkOption {
          type = types.nullOr types.ints.positive;
          default = null;
          description = ''
            Run the command every N seconds. On launchd this becomes
            `StartInterval`; on systemd a companion `.timer` unit drives a
            oneshot service. Mutually exclusive in spirit with a long-running
            `restart = "always"` daemon.
          '';
        };

        standardOutPath = mkOption {
          type = types.nullOr types.str;
          default = null;
          description = "File to append stdout to. Null leaves init-system defaults (journald on Linux).";
        };

        standardErrorPath = mkOption {
          type = types.nullOr types.str;
          default = null;
          description = "File to append stderr to. Null leaves init-system defaults.";
        };

        launchd.config = mkOption {
          type = types.attrs;
          default = { };
          example = {
            ProcessType = "Background";
            Nice = 5;
          };
          description = ''
            Raw launchd plist keys, deep-merged over the generated agent
            config (escape hatch). Use for launchd-only knobs such as
            `ProcessType`, `ThrottleInterval`, or `StartCalendarInterval`.
          '';
        };

        systemd.service = mkOption {
          type = types.attrs;
          default = { };
          example = {
            Service.MemoryMax = "512M";
            Unit.After = [ "network-online.target" ];
          };
          description = ''
            Raw systemd unit sections (`Unit` / `Service` / `Install`),
            deep-merged over the generated service (escape hatch). Use for
            systemd-only knobs such as resource control or hardening.
          '';
        };
      };
    }
  );

  /**
    Render a defaulted service spec to a launchd agent's `config` attrset
    (the plist body home-manager writes for `launchd.agents.<name>.config`).
  */
  toLaunchdConfig =
    svc:
    let
      keepAlive =
        if svc.restart == "always" then
          true
        else if svc.restart == "on-failure" then
          { SuccessfulExit = false; }
        else
          null;

      generated = {
        Label = svc.description;
        ProgramArguments = svc.command;
      }
      // optionalAttrs (svc.environment != { }) { EnvironmentVariables = svc.environment; }
      // optionalAttrs (svc.workingDirectory != null) { WorkingDirectory = svc.workingDirectory; }
      // optionalAttrs (svc.runAtLoad && svc.interval == null) { RunAtLoad = true; }
      // optionalAttrs (keepAlive != null) { KeepAlive = keepAlive; }
      // optionalAttrs (svc.interval != null) { StartInterval = svc.interval; }
      // optionalAttrs (svc.standardOutPath != null) { StandardOutPath = svc.standardOutPath; }
      // optionalAttrs (svc.standardErrorPath != null) { StandardErrorPath = svc.standardErrorPath; };
    in
    lib.recursiveUpdate generated svc.launchd.config;

  /**
    Render one argv element to a systemd `ExecStart` token.

    systemd does not use a shell, so `lib.escapeShellArgs` is wrong here: its
    POSIX `'\''` idiom for embedded quotes would be mis-parsed. Tokens made of
    a conservative safe set pass through unquoted; anything else is
    double-quoted with `"` and `\` backslash-escaped, which systemd's own
    parser understands.

    Known gap: systemd expands `%`-specifiers even inside quotes, so an argv
    element containing `%` is not represented faithfully. Pass such commands
    through the `systemd.service` escape hatch.
  */
  systemdQuoteArg =
    arg:
    if builtins.match "[[:alnum:]_./:=@+-]+" arg != null then
      arg
    else
      "\"" + lib.escape [ "\\" "\"" ] arg + "\"";

  renderExecStart = command: lib.concatMapStringsSep " " systemdQuoteArg command;

  /**
    Render a defaulted service spec to systemd user units. Returns
    `{ service; timer; }` where `service` is always a unit attrset
    (`{ Unit; Service; Install; }`) and `timer` is null unless `interval`
    is set.
  */
  toSystemdUnits =
    svc:
    let
      isTimer = svc.interval != null;

      generatedService = {
        Unit = {
          Description = svc.description;
        };
        Service = {
          ExecStart = renderExecStart svc.command;
          Restart = svc.restart;
        }
        // optionalAttrs (svc.environment != { }) {
          Environment = lib.mapAttrsToList (key: value: "${key}=${value}") svc.environment;
        }
        // optionalAttrs (svc.workingDirectory != null) { WorkingDirectory = svc.workingDirectory; }
        // optionalAttrs isTimer { Type = "oneshot"; }
        // optionalAttrs (svc.standardOutPath != null) { StandardOutput = "append:${svc.standardOutPath}"; }
        // optionalAttrs (svc.standardErrorPath != null) {
          StandardError = "append:${svc.standardErrorPath}";
        };
        # A timer-driven service is started by its `.timer`, not wanted by a
        # target directly; a runAtLoad service is wanted by the session.
        Install = optionalAttrs (svc.runAtLoad && !isTimer) {
          WantedBy = [ "default.target" ];
        };
      };

      generatedTimer =
        if isTimer then
          {
            Unit = {
              Description = "Timer for ${svc.description}";
            };
            Timer = {
              OnBootSec = svc.interval;
              OnUnitActiveSec = svc.interval;
            };
            Install = {
              WantedBy = [ "timers.target" ];
            };
          }
        else
          null;
    in
    {
      service = lib.recursiveUpdate generatedService svc.systemd.service;
      timer = generatedTimer;
    };

  /**
    home-manager module exposing `services.portable.<name>`.

    On Darwin it populates `launchd.agents`; on Linux it populates
    `systemd.user.services` (plus `systemd.user.timers` for interval
    services). The inactive platform's tree is simply not emitted.
  */
  homeModule =
    {
      config,
      pkgs,
      ...
    }:
    let
      enabled = lib.filterAttrs (_: svc: svc.enable) config.services.portable;
      timed = lib.filterAttrs (_: svc: svc.interval != null) enabled;
    in
    {
      options.services.portable = mkOption {
        type = types.attrsOf serviceSubmodule;
        default = { };
        description = ''
          Portable user services. Each entry renders to a native launchd
          agent on macOS and native systemd user units on Linux from a
          single declarative spec.
        '';
      };

      config = lib.mkMerge [
        (lib.mkIf (pkgs.stdenv.hostPlatform.isDarwin && enabled != { }) {
          launchd.agents = lib.mapAttrs (_: svc: {
            enable = true;
            config = toLaunchdConfig svc;
          }) enabled;
        })
        (lib.mkIf (pkgs.stdenv.hostPlatform.isLinux && enabled != { }) {
          systemd.user.services = lib.mapAttrs (_: svc: (toSystemdUnits svc).service) enabled;
          systemd.user.timers = lib.mapAttrs (_: svc: (toSystemdUnits svc).timer) timed;
        })
      ];
    };
in
{
  inherit
    serviceSubmodule
    toLaunchdConfig
    toSystemdUnits
    homeModule
    ;
}
