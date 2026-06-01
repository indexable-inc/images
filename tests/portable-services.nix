# Eval tests for the portable user-service layer. Exercises the pure
# transforms (`toLaunchdConfig` / `toSystemdUnits`) against specs evaluated
# through the real submodule, so defaults and the escape-hatch merge are
# covered without standing up a whole home-manager configuration.
{
  lib,
  pkgs,
  ix,
}:
let
  ps = ix.portableServices;

  # Evaluate a set of service specs through `attrsOf serviceSubmodule` so
  # every default is applied exactly as a real consumer would see it.
  evalServices =
    services:
    (lib.evalModules {
      modules = [
        {
          options.services.portable = lib.mkOption {
            type = lib.types.attrsOf ps.serviceSubmodule;
            default = { };
          };
        }
        { services.portable = services; }
      ];
    }).config.services.portable;

  specs = evalServices {
    # A long-running daemon that always restarts and logs to files.
    daemon = {
      description = "demo daemon";
      command = [
        "/run/current-system/sw/bin/demo"
        "--serve"
      ];
      environment.RUST_LOG = "info";
      restart = "always";
      standardOutPath = "/tmp/demo.out";
      standardErrorPath = "/tmp/demo.err";
    };

    # A one-shot triggered every 300s, plus an escape-hatch key per platform.
    poller = {
      command = [ "/run/current-system/sw/bin/poll" ];
      interval = 300;
      restart = "on-failure";
      launchd.config.ProcessType = "Background";
      systemd.service.Service.MemoryMax = "256M";
    };

    # An argv element with a space exercises systemd-correct quoting.
    quoted = {
      command = [
        "/run/current-system/sw/bin/note"
        "--msg=hello world"
      ];
    };

    # runAtLoad = false on an interval service + an explicit label override:
    # exercises the negative path of both portable improvements.
    quiet = {
      command = [ "/run/current-system/sw/bin/q" ];
      interval = 600;
      runAtLoad = false;
      label = "com.example.quiet";
    };
  };

  daemonLaunchd = ps.toLaunchdConfig specs.daemon;
  daemonSystemd = ps.toSystemdUnits specs.daemon;
  pollerLaunchd = ps.toLaunchdConfig specs.poller;
  pollerSystemd = ps.toSystemdUnits specs.poller;
  quotedSystemd = ps.toSystemdUnits specs.quoted;
  quietLaunchd = ps.toLaunchdConfig specs.quiet;
  quietSystemd = ps.toSystemdUnits specs.quiet;

  assertions = [
    # --- daemon: launchd ---
    {
      assertion =
        daemonLaunchd.ProgramArguments == [
          "/run/current-system/sw/bin/demo"
          "--serve"
        ];
      message = "daemon launchd ProgramArguments mismatch";
    }
    {
      assertion = daemonLaunchd.Label == "org.nix-community.home.daemon";
      message = "daemon launchd Label should default to the name-based home convention";
    }
    {
      assertion = daemonLaunchd.RunAtLoad == true;
      message = "daemon launchd RunAtLoad should be set (runAtLoad default true, no interval)";
    }
    {
      assertion = daemonLaunchd.KeepAlive == true;
      message = "daemon restart=always should map to launchd KeepAlive = true";
    }
    {
      assertion = daemonLaunchd.EnvironmentVariables == { RUST_LOG = "info"; };
      message = "daemon launchd EnvironmentVariables mismatch";
    }
    {
      assertion = daemonLaunchd.StandardOutPath == "/tmp/demo.out";
      message = "daemon launchd StandardOutPath mismatch";
    }
    {
      assertion = !(daemonLaunchd ? StartInterval);
      message = "daemon without interval must not emit StartInterval";
    }

    # --- daemon: systemd ---
    {
      assertion = daemonSystemd.service.Service.ExecStart == "/run/current-system/sw/bin/demo --serve";
      message = "daemon systemd ExecStart should pass safe argv tokens through unquoted";
    }
    {
      assertion = daemonSystemd.service.Service.Restart == "always";
      message = "daemon restart=always should map to systemd Restart = always";
    }
    {
      assertion = daemonSystemd.service.Service.Environment == [ "RUST_LOG=info" ];
      message = "daemon systemd Environment should be K=V list";
    }
    {
      assertion = daemonSystemd.service.Install.WantedBy == [ "default.target" ];
      message = "runAtLoad non-timer service should be WantedBy default.target";
    }
    {
      assertion = daemonSystemd.timer == null;
      message = "daemon without interval must not produce a timer";
    }

    # --- poller: launchd (interval => StartInterval + RunAtLoad via runAtLoad) ---
    {
      assertion = pollerLaunchd.StartInterval == 300;
      message = "poller launchd StartInterval mismatch";
    }
    {
      assertion = pollerLaunchd.RunAtLoad == true;
      message = "interval service should honor runAtLoad (default true) alongside StartInterval";
    }
    {
      assertion = pollerLaunchd.KeepAlive == { SuccessfulExit = false; };
      message = "restart=on-failure should map to launchd KeepAlive.SuccessfulExit = false";
    }
    {
      assertion = pollerLaunchd.ProcessType == "Background";
      message = "launchd escape-hatch key ProcessType should be merged in";
    }

    # --- poller: systemd (interval => oneshot service + timer) ---
    {
      assertion = pollerSystemd.service.Service.Type == "oneshot";
      message = "interval service should be systemd Type = oneshot";
    }
    {
      assertion = !(pollerSystemd.service ? Install) || pollerSystemd.service.Install == { };
      message = "interval service must not be WantedBy default.target (driven by timer)";
    }
    {
      assertion = pollerSystemd.timer != null && pollerSystemd.timer.Timer.OnUnitActiveSec == 300;
      message = "poller should produce a timer with OnUnitActiveSec = interval";
    }
    {
      assertion =
        pollerSystemd.timer.Timer.OnActiveSec == "1s" && !(pollerSystemd.timer.Timer ? OnBootSec);
      message = "runAtLoad interval service: timer fires promptly via OnActiveSec, not OnBootSec";
    }
    {
      assertion = pollerSystemd.timer.Install.WantedBy == [ "timers.target" ];
      message = "poller timer should be WantedBy timers.target";
    }
    {
      assertion = pollerSystemd.service.Service.MemoryMax == "256M";
      message = "systemd escape-hatch key MemoryMax should be merged in";
    }

    # --- quoted: systemd-correct quoting of an arg with whitespace ---
    {
      assertion =
        quotedSystemd.service.Service.ExecStart == "/run/current-system/sw/bin/note \"--msg=hello world\"";
      message = "argv element with a space should be systemd double-quoted, not shell-escaped";
    }

    # --- quiet: runAtLoad=false interval + explicit label override ---
    {
      assertion = quietLaunchd.Label == "com.example.quiet";
      message = "explicit label should override the name-based default";
    }
    {
      assertion = !(quietLaunchd ? RunAtLoad);
      message = "runAtLoad=false must not set RunAtLoad on launchd";
    }
    {
      assertion = quietSystemd.timer.Timer.OnBootSec == 600 && !(quietSystemd.timer.Timer ? OnActiveSec);
      message = "runAtLoad=false timer should wait OnBootSec, not fire via OnActiveSec";
    }
  ];

  failures = map (a: a.message) (lib.filter (a: !a.assertion) assertions);
in
assert lib.assertMsg (failures == [ ]) (
  "portable-services:\n  " + lib.concatStringsSep "\n  " failures
);
pkgs.runCommand "ix-test-portable-services" { } ''
  mkdir -p "$out"
''
