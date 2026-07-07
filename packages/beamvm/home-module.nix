# home-manager module exposing `services.beamvm`: persistent BEAM VMs as user
# services (native launchd agent on macOS, systemd user unit on Linux via
# portable-services), with the OTP applications each VM hosts declared in Nix
# and hot-swapped on switch instead of restarted.
#
# The no-restart contract, split across three layers so it holds by
# construction rather than by detection:
#
#   * The unit's command line references only the beamvm package and stable
#     $XDG paths, never a tenant's store path, so updating an app cannot
#     change the unit definition and cannot restart the VM.
#   * The manifest of apps lives at a stable xdg.configFile path whose symlink
#     home-manager rewrites at switch; an activation hook then pokes the VM's
#     control socket and the harness converges live (add/remove apps, swap
#     changed modules in place; see packages/beamvm/harness.ex).
#   * Updating the beamvm package itself (new harness code or a new
#     Erlang/Elixir toolchain) DOES change the unit and restarts it: exactly
#     the one case where hot reload is impossible (a new ERTS cannot be
#     entered from a running VM).
{
  indexPackages,
  portableServicesModule,
  ix,
}: {
  config,
  lib,
  pkgs,
  ...
}: let
  inherit
    (lib)
    getExe'
    mkIf
    mkOption
    optionalString
    types
    ;

  cfg = config.services.beamvm;

  defaultPackage = (indexPackages pkgs.stdenv.hostPlatform.system).beamvm;

  jsonFormat = pkgs.formats.json {};

  stateDirFor = name: vm:
    if vm.stateDir != null
    then toString vm.stateDir
    else "${config.xdg.stateHome}/beamvm/${name}";

  manifestTargetFor = name: "beamvm/${name}/manifest.json";

  manifestFor = name: vm:
    jsonFormat.generate "beamvm-${name}-manifest.json" {
      apps =
        lib.mapAttrs (_appName: app: {
          # Globs, expanded by the harness: a release's `lib/<dep>-<vsn>/ebin`
          # layout is only enumerable after the package builds, and expanding
          # here would be import-from-derivation.
          code_path_globs = ["${app.package}/lib/*/ebin"] ++ app.extraCodePathGlobs;
          inherit (app) start;
          # sys.config carries the baked build-time config (config.exs +
          # prod.exs; `server: true` for Phoenix lives there), runtime.exs the
          # boot-time env reads; the harness applies them in that order, as a
          # release boot would.
          sys_config_globs =
            lib.optional app.releaseRuntimeConfig "${app.package}/releases/*/sys.config";
          runtime_config_globs =
            lib.optional app.releaseRuntimeConfig "${app.package}/releases/*/runtime.exs";
        })
        vm.apps;
    };

  appSubmodule = types.submodule {
    options = {
      package = mkOption {
        type = types.package;
        description = ''
          Mix release providing this application's compiled code
          (`lib/<app>-<vsn>/ebin` for the app and every runtime dep). The
          harness skips release-bundled toolchain libraries (elixir, stdlib,
          logger, ...) that the VM already loaded, so releases built against
          the same pinned toolchain as beamvm compose safely.
        '';
      };

      start = mkOption {
        type = types.bool;
        default = true;
        description = ''
          Start the application (ensure_all_started) after loading its code.
          False loads code only, for library tenants another app calls into.
        '';
      };

      releaseRuntimeConfig = mkOption {
        type = types.bool;
        default = true;
        description = ''
          Replay the release's `releases/*/runtime.exs` (the config provider
          a release boot script would run) before starting the app. Harmless
          when the release has none.
        '';
      };

      extraCodePathGlobs = mkOption {
        type = types.listOf types.str;
        default = [];
        description = "Additional ebin-dir globs appended to the release's own.";
      };
    };
  };

  vmSubmodule = types.submodule {
    options = {
      apps = mkOption {
        type = types.attrsOf appSubmodule;
        default = {};
        description = "OTP applications this VM loads and runs, keyed by application name.";
      };

      environment = mkOption {
        type = types.attrsOf types.str;
        default = {};
        description = ''
          Environment variables for the VM process (shared by every app in
          it). Rendered into the world-readable unit; secrets go through
          environmentFile or secretsCommand instead.

          Changing these does NOT hot-reload: the environment lives in the
          unit definition, so the portable-services layer restarts the VM.
          That is the honest semantics -- OS process env cannot be swapped
          from inside.
        '';
      };

      environmentFile = mkOption {
        type = types.nullOr types.path;
        default = null;
        description = ''
          KEY=VALUE secrets file sourced by the launch wrapper at start
          (launchd has no systemd EnvironmentFile). Point it at a runtime
          path owned by your secret manager.
        '';
      };

      secretsCommand = mkOption {
        type = types.nullOr (types.listOf types.str);
        default = null;
        description = ''
          Optional command wrapping the harness exec to inject secrets
          (Bitwarden `bws run -- ...` or any CLI that execs its trailing
          arguments). Put the injector on PATH via extraPath.
        '';
      };

      extraPath = mkOption {
        type = types.listOf types.package;
        default = [];
        description = "Extra packages on the VM's PATH, for apps that shell out.";
      };

      stateDir = mkOption {
        type = types.nullOr types.path;
        default = null;
        description = ''
          Directory for the control socket. Null uses
          `$XDG_STATE_HOME/beamvm/<name>`.
        '';
      };
    };
  };

  # The repo's checked-bash writer, same shape as the symphony launcher: an
  # exec-style wrapper (POSIX `.` sourcing of the secrets file, exec handoff
  # to the optional secrets injector) is the sanctioned bash escape hatch.
  # `runtimeInputs` prepends extraPath to PATH for apps that shell out.
  launcherFor = name: vm:
    ix.writeBashApplication pkgs {
      name = "beamvm-${name}-launch";
      runtimeInputs = vm.extraPath;
      text =
        optionalString (vm.environmentFile != null) ''
          # launchd has no EnvironmentFile, so the wrapper sources the
          # secrets file itself. set -a exports everything it sets.
          set -a
          # shellcheck disable=SC1090
          . ${lib.escapeShellArg (toString vm.environmentFile)}
          set +a
        ''
        + ''
          export BEAMVM_STATE_DIR=${lib.escapeShellArg (stateDirFor name vm)}
          export BEAMVM_MANIFEST=${lib.escapeShellArg "${config.xdg.configHome}/${manifestTargetFor name}"}
          exec ${
            optionalString (vm.secretsCommand != null) (lib.escapeShellArgs vm.secretsCommand + " ")
          }${getExe' cfg.package "beamvm-harness"}
        '';
    };
in {
  # Both homeModules.beamvm and homeModules.symphony (which composes this
  # module for its beamvm runtime) import this file through fresh `import`
  # calls; the module-system key dedups those into one option declaration.
  key = "index:packages/beamvm/home-module.nix";

  imports = [portableServicesModule];

  options.services.beamvm = {
    package = mkOption {
      type = types.package;
      default = defaultPackage;
      defaultText = lib.literalExpression "index.packages.\${system}.beamvm";
      description = ''
        The beamvm harness + ctl package. Updating it (or the Erlang/Elixir
        toolchain inside it) restarts every VM; that is the one update class
        hot reload cannot cover.
      '';
    };

    vms = mkOption {
      type = types.attrsOf vmSubmodule;
      default = {};
      description = "Persistent BEAM VMs to run, keyed by VM name.";
    };
  };

  config = mkIf (cfg.vms != {}) {
    xdg.configFile =
      lib.mapAttrs' (
        name: vm:
          lib.nameValuePair (manifestTargetFor name) {source = manifestFor name vm;}
      )
      cfg.vms;

    services.portable =
      lib.mapAttrs' (
        name: vm:
          lib.nameValuePair "beamvm-${name}" {
            description = "beamvm ${name}: persistent BEAM VM";
            command = [(lib.getExe (launcherFor name vm))];
            # A defined, writable cwd instead of wherever the init system
            # spawns us: BEAM apps commonly derive file defaults from cwd
            # (symphony's disk log handler writes cwd-relative
            # `log/symphony.log` when no explicit :log_file env is set), and
            # $HOME must not silently collect those. Created by the
            # activation hook below; a missing WorkingDirectory fails the
            # spawn on both init systems.
            workingDirectory = stateDirFor name vm;
            inherit (vm) environment;
            # The VM hosts long-running supervision trees; the unit's whole
            # job is keeping it up from login onward.
            restart = "always";
            runAtLoad = true;
          }
      )
      cfg.vms;

    # After home-manager rewrites the manifest symlinks (and after the
    # platform's unit-management step handled any unit-definition changes),
    # poke each running VM to converge on the new manifest. "Not running"
    # (exit 2) is fine -- the next start reads the same stable path -- but a
    # running VM that fails to reload fails the switch loudly.
    # VM names reach systemd/launchd unit names, socket paths, and this
    # activation script; constrain them once here rather than letting a
    # metacharacter render an invalid unit or script.
    assertions =
      lib.mapAttrsToList (name: _vm: {
        assertion = builtins.match "[A-Za-z0-9_-]+" name != null;
        message = "services.beamvm.vms.\"${name}\": VM names must match [A-Za-z0-9_-]+";
      })
      cfg.vms;

    home.activation =
      # State dirs first: the unit's WorkingDirectory must exist before the
      # platform service step (re)starts anything, and the harness only
      # creates it AFTER spawn, which is too late for the cwd.
      lib.mapAttrs' (
        name: vm:
          lib.nameValuePair "beamvmStateDir-${name}" (
            lib.hm.dag.entryBetween ["reloadSystemd" "setupLaunchAgents"] ["writeBoundary"] ''
              run mkdir -p ${lib.escapeShellArg (stateDirFor name vm)}
            ''
          )
      )
      cfg.vms
      // lib.mapAttrs' (
        name: vm:
          lib.nameValuePair "beamvmReload-${name}" (
            # Plain statements, no name-derived shell identifiers: the VM
            # name appears only inside quoted strings.
            lib.hm.dag.entryAfter ["linkGeneration" "reloadSystemd" "setupLaunchAgents"] ''
              beamvmReloadRc=0
              run ${getExe' cfg.package "beamvm-ctl"} \
                --socket ${lib.escapeShellArg "${stateDirFor name vm}/control.sock"} \
                reload || beamvmReloadRc=$?
              if [ "$beamvmReloadRc" -eq 2 ]; then
                verboseEcho ${lib.escapeShellArg "beamvm ${name}: not running; next start reads the current manifest"}
              elif [ "$beamvmReloadRc" -ne 0 ]; then
                echo ${lib.escapeShellArg "beamvm ${name}: hot reload failed"} "(exit $beamvmReloadRc)" >&2
                exit "$beamvmReloadRc"
              fi
            ''
          )
      )
      cfg.vms;
  };
}
