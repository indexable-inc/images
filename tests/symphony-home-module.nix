# Eval tests for the symphony home-manager module. Evaluates
# `services.symphony` through the real module with a stub portable-services
# declaration (options only, no platform dispatch), then renders the
# resulting `services.portable.symphony` spec through the pure launchd and
# systemd transforms. The platform dispatch itself is already covered by
# tests/portable-services.nix, so asserting on the transforms keeps this
# test host-independent: both the plist and the user unit are checked on
# every CI platform.
{
  lib,
  pkgs,
  ix,
  paths,
}: let
  ps = ix.portableServices;

  # Same `services.portable` declaration the real home module provides,
  # minus its launchd/systemd config, so evalModules needs no home-manager
  # option stubs.
  portableDecl = {
    options.services.portable = lib.mkOption {
      type = lib.types.attrsOf ps.serviceSubmodule;
      default = {};
    };
  };

  # Eval-only stand-ins: lib.getExe and the manifest render only construct
  # store-path strings, so the stubs are instantiated but never built. The
  # release/root passthrus mirror the real package's (home-module defaults
  # dereference them).
  releaseStub = pkgs.runCommand "symphony-release" {} ''
    mkdir -p "$out"
  '';
  symphonyStub =
    pkgs.runCommand "symphony" {
      meta.mainProgram = "symphony";
      passthru = {
        release = releaseStub;
        root = "/stub/symphony-root";
        runtimeTools = [pkgs.jq];
      };
    } ''
      mkdir -p "$out/bin"
    '';

  # Options-only stand-in for the beamvm home-module, mirroring
  # portableDecl: the mapping from services.symphony onto services.beamvm is
  # what this test owns; the beamvm module's own rendering (manifest, unit,
  # activation) is exercised by its package checks and real home-manager
  # evals.
  beamvmDecl = {
    options.services.beamvm = {
      package = lib.mkOption {
        type = lib.types.package;
        default = pkgs.hello;
      };
      vms = lib.mkOption {
        type = lib.types.attrsOf lib.types.anything;
        default = {};
      };
    };
  };

  # The beamvm branch anchors the default state dir at xdg.stateHome and
  # writes the catalog symlink through xdg.configFile, both of which
  # home-manager normally declares.
  xdgDecl = {
    options.xdg = {
      stateHome = lib.mkOption {
        type = lib.types.str;
        default = "/home/dev/.local/state";
      };
      configHome = lib.mkOption {
        type = lib.types.str;
        default = "/home/dev/.config";
      };
      configFile = lib.mkOption {
        type = lib.types.attrsOf lib.types.anything;
        default = {};
      };
    };
  };

  homeModule = import (paths.root + "/packages/agent/symphony/home-module.nix") {
    indexPackages = _system: {symphony = symphonyStub;};
    portableServicesModule = portableDecl;
    beamvmModule = beamvmDecl;
    inherit ix;
  };

  evalSymphony = settings:
    (lib.evalModules {
      modules = [
        # Inject `pkgs` the way home-manager does (a module arg), not via
        # specialArgs.
        {_module.args.pkgs = pkgs;}
        xdgDecl
        homeModule
        {services.symphony = settings;}
      ];
    }).config;

  # Full configuration: every knob that changes the rendered units.
  full = evalSymphony {
    enable = true;
    # These assertions cover the dedicated-unit render; the beamvm mapping
    # has its own set below.
    runtime = "standalone";
    httpPort = 4141;
    stateDir = "/home/dev/.local/state/symphony";
    primaryRepo = "/home/dev/src/index";
    packDir = "/home/dev/src/symphony-pack";
    extraEnvironment.SYMPHONY_BOT_USERNAME = "symphony-bot";
    environmentFile = "/home/dev/.config/symphony/secrets.env";
    secretsCommand = [
      "bws"
      "run"
      "--"
    ];
    extraPath = [pkgs.jq];
  };

  # Defaults: only enable set, so the launcher owns state-dir and pack
  # resolution and the wrapper is a bare exec.
  minimal = evalSymphony {
    enable = true;
    runtime = "standalone";
  };

  # The default runtime: the compiled release hosted in the persistent VM.
  beamvm = evalSymphony {enable = true;};
  beamvmVm = beamvm.services.beamvm.vms.symphony;

  fullSpec = full.services.portable.symphony;
  fullLaunchd = ps.toLaunchdConfig fullSpec;
  fullSystemd = ps.toSystemdUnits fullSpec;
  fullLauncher = full.services.symphony.launcher;

  minimalSpec = minimal.services.portable.symphony;
  minimalLauncher = minimal.services.symphony.launcher;

  # lib.hasInfix compiles the needle into a regex, and Nix refuses a regex
  # string that carries store-path context; the needles below interpolate
  # derivations (jq, the symphony stub), so drop the context. Assertion-only:
  # nothing built depends on the discarded context.
  hasDrvInfix = needle: lib.hasInfix (builtins.unsafeDiscardStringContext needle);

  assertions = [
    # --- command: one argv element, the rendered launch wrapper ---
    {
      assertion =
        lib.length fullSpec.command == 1 && lib.hasSuffix "/bin/symphony-launch" (lib.head fullSpec.command);
      message = "command should be exactly the launch wrapper";
    }

    # --- environment: typed options land in both renders ---
    {
      assertion = fullLaunchd.EnvironmentVariables.SYMPHONY_HTTP_PORT == "4141";
      message = "httpPort should render into the launchd plist environment";
    }
    {
      assertion = fullLaunchd.EnvironmentVariables.SYMPHONY_PACK_DIR == "/home/dev/src/symphony-pack";
      message = "packDir should render as SYMPHONY_PACK_DIR (the hot-reload surface)";
    }
    {
      assertion = fullLaunchd.EnvironmentVariables.SYMPHONY_STATE_DIR == "/home/dev/.local/state/symphony";
      message = "stateDir should render as SYMPHONY_STATE_DIR when set";
    }
    {
      assertion = fullLaunchd.EnvironmentVariables.SYMPHONY_BOT_USERNAME == "symphony-bot";
      message = "extraEnvironment should merge into the launchd environment";
    }
    {
      assertion =
        lib.elem "SYMPHONY_PRIMARY_REPO=/home/dev/src/index" fullSystemd.service.Service.Environment
        && lib.elem "SYMPHONY_BOT_USERNAME=symphony-bot" fullSystemd.service.Service.Environment;
      message = "typed options and extraEnvironment should land in the systemd Environment list";
    }

    # --- restart policy: the BEAM is the scheduler, keep it up ---
    {
      assertion = fullLaunchd.KeepAlive == true && fullLaunchd.RunAtLoad == true;
      message = "launchd agent should KeepAlive and RunAtLoad (restart=always daemon)";
    }
    {
      assertion = !(fullLaunchd ? StartInterval);
      message = "no interval: cron lives inside the runtime, not the unit";
    }
    {
      assertion = fullSystemd.service.Service.Restart == "always";
      message = "systemd unit should Restart=always";
    }
    {
      assertion = fullSystemd.service.Install.WantedBy == ["default.target"] && fullSystemd.timer == null;
      message = "systemd unit should be session-wanted with no timer";
    }

    # --- launch wrapper: PATH, secrets file, secretsCommand ---
    {
      assertion = hasDrvInfix "${pkgs.jq}/bin" fullLauncher.text;
      message = "extraPath entries should be prepended to PATH in the wrapper";
    }
    {
      # escapeShellArg leaves safe paths unquoted, so match the bare form.
      assertion = lib.hasInfix ". /home/dev/.config/symphony/secrets.env" fullLauncher.text;
      message = "environmentFile should be sourced by the wrapper";
    }
    {
      assertion = hasDrvInfix "exec bws run -- ${symphonyStub}/bin/symphony" fullLauncher.text;
      message = "secretsCommand should wrap the exec of the package launcher";
    }

    # --- defaults: launcher-owned state dir, bare exec wrapper ---
    {
      assertion =
        minimalSpec.environment
        == {
          SYMPHONY_HTTP_PORT = "4040";
          SYMPHONY_WORKFLOW_PACK = "example";
        };
      message = "defaults should export only port and built-in pack (launcher owns SYMPHONY_STATE_DIR)";
    }
    {
      assertion =
        hasDrvInfix "exec ${symphonyStub}/bin/symphony \"$@\"" minimalLauncher.text
        && !(lib.hasInfix "set -a" minimalLauncher.text);
      message = "without environmentFile/secretsCommand the wrapper should be a bare exec";
    }

    # --- beamvm runtime (the default): mapping onto services.beamvm ---
    {
      assertion = !(beamvm.services.portable ? symphony);
      message = "beamvm runtime should not also render the standalone unit";
    }
    {
      assertion = beamvmVm.apps.symphony_elixir.package == releaseStub;
      message = "beamvm runtime should host the compiled release (package.release)";
    }
    {
      # The stable config symlink, NOT the store path: a store path in the
      # unit environment would restart the VM on every symphony update.
      assertion = beamvmVm.environment.SYMPHONY_ROOT == "/home/dev/.config/symphony/root";
      message = "SYMPHONY_ROOT should go through the stable config symlink";
    }
    {
      assertion = beamvm.xdg.configFile."symphony/root".source == "/stub/symphony-root";
      message = "the catalog symlink should retarget to the package root at switch";
    }
    {
      assertion = lib.elem pkgs.jq beamvmVm.extraPath;
      message = "the package's runtimeTools should reach the VM PATH (workflow scripts shell out)";
    }
    {
      # config.ex mkdir_p!'s these; a store-rooted default would crash, so
      # every writable dir must be anchored explicitly under the state dir.
      assertion =
        beamvmVm.environment.SYMPHONY_STATE_DIR
        == "/home/dev/.local/state/symphony"
        && beamvmVm.environment.SYMPHONY_WORKSPACES_DIR == "/home/dev/.local/state/symphony/workspaces"
        && beamvmVm.environment.SYMPHONY_RUNS_DIR == "/home/dev/.local/state/symphony/runs"
        && beamvmVm.environment.SYMPHONY_LOGS_ROOT == "/home/dev/.local/state/symphony/log";
      message = "beamvm runtime should anchor every writable dir under the state dir";
    }
    {
      assertion = beamvmVm.environment.SYMPHONY_HTTP_PORT == "4040";
      message = "beamvm runtime should keep env parity for the HTTP port";
    }
  ];

  failures = map (a: a.message) (lib.filter (a: !a.assertion) assertions);
in
  assert lib.assertMsg (failures == []) (
    "symphony-home-module:\n  " + lib.concatStringsSep "\n  " failures
  );
    pkgs.runCommand "ix-test-symphony-home-module" {__structuredAttrs = true;} ''
      mkdir -p "$out"
    ''
