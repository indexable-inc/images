# home-manager module exposing `services.symphony`: run the Symphony BEAM
# runtime as a user service, composing the portable-services layer so one
# spec renders a native launchd agent on macOS and a native systemd user
# unit on Linux. Option vocabulary mirrors the NixOS module
# (modules/services/symphony/default.nix); only the concepts that make
# sense in a user session are kept (no user/tmpfiles/polkit).
#
# Hot reload: point `packDir` at a mutable checkout (a plain working tree,
# not a store path); the runtime re-reads `.sym` workflows and skills from
# it live, so a `git pull` in the pack updates behavior without a restart.
#
# Secrets never land in the Nix store: `extraEnvironment` is rendered into
# the world-readable agent/unit, so real credentials go through
# `environmentFile` (sourced by the launch wrapper at start; launchd has no
# EnvironmentFile equivalent) or `secretsCommand` (a `bws run --`-style
# injector that wraps the exec).
{
  indexPackages,
  portableServicesModule,
  beamvmModule,
  ix,
}: {
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
    optionalAttrs
    optionalString
    types
    ;

  cfg = config.services.symphony;

  defaultPackage = (indexPackages pkgs.stdenv.hostPlatform.system).symphony;

  # The repo's checked-bash writer (lib/util/writers.nix): the launcher is an
  # exec-style wrapper (POSIX `.` sourcing of the secrets file, `exec` handoff
  # to the optional secrets injector), which is the sanctioned bash escape
  # hatch. `runtimeInputs` prepends `extraPath` to PATH, which is how the
  # host-owned `codex` (deliberately not a runtime input of the symphony
  # package) reaches the service.
  inherit (ix) writeBashApplication;

  launcher = writeBashApplication pkgs {
    name = "symphony-launch";
    runtimeInputs = cfg.extraPath;
    text =
      optionalString (cfg.environmentFile != null) ''
        # launchd has no EnvironmentFile, so the wrapper sources the secrets
        # file itself: KEY=VALUE lines, a shell-sourceable superset of the
        # systemd EnvironmentFile format. set -a exports everything it sets.
        set -a
        . ${lib.escapeShellArg (toString cfg.environmentFile)}
        set +a
      ''
      + ''
        exec ${
          optionalString (cfg.secretsCommand != null) (lib.escapeShellArgs cfg.secretsCommand + " ")
        }${lib.getExe cfg.package} "$@"
      '';
  };
in {
  imports = [
    portableServicesModule
    beamvmModule
  ];

  options.services.symphony = {
    enable = mkEnableOption "the Symphony runtime as a user service";

    package = mkOption {
      type = types.package;
      default = defaultPackage;
      defaultText = lib.literalExpression "index.packages.\${system}.symphony";
      description = "Symphony package to run (this flake's launcher around bin/run-nix).";
    };

    runtime = mkOption {
      type = types.enum [
        "beamvm"
        "standalone"
      ];
      default = "beamvm";
      description = ''
        How the runtime is hosted.

        "beamvm" (the default) runs the compiled release inside the
        persistent BEAM VM (services.beamvm): no compile at boot, and a
        symphony update hot-swaps code in the running VM at switch time --
        no restart, no dropped LiveView sockets or in-flight runs. Only a
        beamvm/toolchain update restarts the VM.

        "standalone" is the original path: a dedicated unit whose launcher
        stages the source tree and runs `mix run --no-halt`, recompiling at
        every start. Updates restart the unit.
      '';
    };

    releasePackage = mkOption {
      type = types.nullOr types.package;
      default = null;
      defaultText = lib.literalExpression "config.services.symphony.package.release";
      description = ''
        Compiled mix release the beamvm runtime code-loads. Null follows
        `package` (its passthru.release), so overriding `package` alone
        keeps code and catalogs from the same build.
      '';
    };

    stateDir = mkOption {
      type = types.nullOr types.path;
      default = null;
      description = ''
        Directory for runs, workspaces, logs, and the staged runtime copy
        (SYMPHONY_STATE_DIR). Null leaves it to the launcher, which resolves
        `$HOME/.local/state/symphony` at start; an eval-time default would
        have to bake one machine's home path into the unit.
      '';
    };

    httpPort = mkOption {
      type = types.port;
      default = 4040;
      description = "Phoenix HTTP listener port (SYMPHONY_HTTP_PORT).";
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
      description = ''
        Absolute path to an external workflow pack (SYMPHONY_PACK_DIR).
        Takes precedence over workflowPack. This is the hot-reload surface:
        point it at a mutable checkout (as a string path, never a `./`
        literal, which would copy an immutable snapshot into the store) and
        the runtime picks up edited `.sym` workflows and skills without a
        restart.
      '';
    };

    extraEnvironment = mkOption {
      type = types.attrsOf types.str;
      default = {};
      description = ''
        Additional environment variables exported to the service. Use for
        non-secret config (SYMPHONY_BOT_USERNAME, LINEAR_WORKSPACE_SLUG,
        ...); values are rendered into the world-readable Nix store, so put
        secrets in environmentFile or behind secretsCommand instead.
      '';
    };

    environmentFile = mkOption {
      type = types.nullOr types.path;
      default = null;
      description = ''
        Path to a KEY=VALUE secrets file (LINEAR_API_KEY, GITHUB_TOKEN,
        SLACK_BOT_OAUTH_TOKEN, ...) sourced by the launch wrapper at start,
        since launchd has no systemd EnvironmentFile. Point it at a runtime
        path owned by your secret manager; leave null if secretsCommand
        injects everything.
      '';
    };

    secretsCommand = mkOption {
      type = types.nullOr (types.listOf types.str);
      default = null;
      example = [
        "bws"
        "run"
        "--"
      ];
      description = ''
        Optional command that wraps the exec and injects secrets into the
        environment (Bitwarden `bws run -- ...` or any compatible CLI that
        execs its trailing arguments). Put the injector binary on PATH via
        extraPath, and any token it needs (BWS_ACCESS_TOKEN) in
        environmentFile.
      '';
    };

    extraPath = mkOption {
      type = types.listOf types.package;
      default = [];
      example = lib.literalExpression "[ pkgs.jq pkgs.gh ]";
      description = ''
        Extra packages prepended to the service PATH. bin/run-nix refuses to
        start without an authenticated `codex` on PATH (intentionally not a
        runtime input of the symphony package, so the binary and its
        credentials stay host-owned); deployers add their codex package here,
        plus anything workflows shell out to.
      '';
    };

    launcher = mkOption {
      type = types.package;
      readOnly = true;
      description = ''
        The rendered launch wrapper: sources environmentFile, prepends
        extraPath to PATH, and execs the package under secretsCommand.
        Read-only; exposed so tests can inspect it and operators can run the
        exact service command by hand.
      '';
    };
  };

  config = mkIf cfg.enable (lib.mkMerge [
    {services.symphony.launcher = launcher;}
    (mkIf (cfg.runtime == "beamvm") {
      # Catalog tree indirection: home-manager retargets this symlink at
      # switch, so the VM (whose unit env carries only the stable path)
      # reads updated catalogs without a unit change.
      xdg.configFile."symphony/root".source = cfg.package.root;

      # The persistent-VM runtime: SYMPHONY_* env parity with bin/run-nix,
      # except SYMPHONY_ROOT points read-only at the store catalogs (the
      # release needs no writable staging copy) and every writable dir is
      # anchored explicitly under the state dir -- config.ex mkdir_p!'s its
      # dirs, which must never resolve to a store default.
      services.beamvm.vms.symphony = {
        apps.symphony_elixir.package =
          if cfg.releasePackage != null
          then cfg.releasePackage
          else cfg.package.release;
        inherit (cfg) environmentFile secretsCommand;
        # The package's own runtime tool set first (ExecRunner inherits this
        # PATH for workflow scripts; the bundled pack shells out to git/gh/
        # jq), then deployment extras (codex lives there).
        extraPath = cfg.package.runtimeTools ++ cfg.extraPath;
        environment = let
          stateDir =
            if cfg.stateDir != null
            then toString cfg.stateDir
            else "${config.xdg.stateHome}/symphony";
        in
          {
            # Via the stable config symlink (written below), NOT the store
            # path: a store path in the unit environment would change the
            # unit on every symphony update and restart the VM, defeating
            # the hot-reload contract this runtime exists for.
            SYMPHONY_ROOT = "${config.xdg.configHome}/symphony/root";
            SYMPHONY_STATE_DIR = stateDir;
            SYMPHONY_WORKSPACES_DIR = "${stateDir}/workspaces";
            SYMPHONY_RUNS_DIR = "${stateDir}/runs";
            SYMPHONY_LOGS_ROOT = "${stateDir}/log";
            SYMPHONY_HTTP_PORT = toString cfg.httpPort;
            SYMPHONY_WORKFLOW_PACK = cfg.workflowPack;
          }
          // optionalAttrs (cfg.primaryRepo != null) {
            SYMPHONY_PRIMARY_REPO = toString cfg.primaryRepo;
          }
          // optionalAttrs (cfg.repoRoot != null) {
            SYMPHONY_REPO_ROOT = toString cfg.repoRoot;
          }
          // optionalAttrs (cfg.packDir != null) {
            SYMPHONY_PACK_DIR = toString cfg.packDir;
          }
          // cfg.extraEnvironment;
      };
    })
    (mkIf (cfg.runtime == "standalone") {
      services.portable.symphony = {
        description = "Symphony runtime";
        command = [(lib.getExe launcher)];
        environment =
          {
            SYMPHONY_HTTP_PORT = toString cfg.httpPort;
            SYMPHONY_WORKFLOW_PACK = cfg.workflowPack;
          }
          // optionalAttrs (cfg.stateDir != null) {
            SYMPHONY_STATE_DIR = toString cfg.stateDir;
          }
          // optionalAttrs (cfg.primaryRepo != null) {
            SYMPHONY_PRIMARY_REPO = toString cfg.primaryRepo;
          }
          // optionalAttrs (cfg.repoRoot != null) {
            SYMPHONY_REPO_ROOT = toString cfg.repoRoot;
          }
          // optionalAttrs (cfg.packDir != null) {
            SYMPHONY_PACK_DIR = toString cfg.packDir;
          }
          // cfg.extraEnvironment;
        # The BEAM is the scheduler: cron triggers live inside the runtime,
        # so the unit's whole job is to keep it up from login onward. No
        # interval; a poller here would fight the long-running daemon.
        restart = "always";
        runAtLoad = true;
      };
    })
  ]);
}
