# Eval test for the darwin loopback localhost proxy module (#2988).
# Evaluates modules/darwin/localhost-proxy.nix against stub nix-darwin
# options and asserts the wiring: dnsmasq answers *.localhost on loopback and
# forwards upstream to MagicDNS, Caddy serves the vhost registry from a
# build-time-validated Caddyfile, and the optional Wi-Fi DNS owner restores
# automatic DNS when launchd unloads it. The returned check derivation depends
# on the generated artifacts and checks their lifecycle commands.
{
  lib,
  pkgs,
  paths,
  writeBashApplication,
}: let
  launchdDaemonType = lib.types.submodule {
    options.serviceConfig = lib.mkOption {
      type = lib.types.attrsOf lib.types.raw;
      default = {};
    };
  };

  optionStubs = {
    options.launchd.daemons = lib.mkOption {
      type = lib.types.attrsOf launchdDaemonType;
      default = {};
    };
  };

  eval = extraModule:
    (lib.evalModules {
      modules = [
        optionStubs
        (import (paths.root + "/modules/darwin/localhost-proxy.nix") {inherit writeBashApplication;})
        {_module.args.pkgs = pkgs;}
        extraModule
      ];
    }).config;

  disabled = eval {};
  enabled = eval {
    services.localhostProxy = {
      enable = true;
      services = {
        nwm = 7532;
        weave = 7677;
      };
    };
  };
  wifiDns = eval {
    services.localhostProxy = {
      enable = true;
      services.nwm = 7532;
      setWifiDns = true;
    };
  };

  dnsmasqArgs = enabled.launchd.daemons.dnsmasq.serviceConfig.ProgramArguments;
  wifiDnsmasqArgs = wifiDns.launchd.daemons.dnsmasq.serviceConfig.ProgramArguments;
  caddyArgs = enabled.launchd.daemons.caddy.serviceConfig.ProgramArguments;
  dnsmasqEntrypoint = builtins.head dnsmasqArgs;
  wifiDnsmasqEntrypoint = builtins.head wifiDnsmasqArgs;
  # The element after `--config`: the validated Caddyfile store path. Built
  # by the check below so a Caddyfile `caddy validate` rejects fails CI.
  caddyfile = builtins.elemAt caddyArgs (
    1 + lib.lists.findFirstIndex (arg: arg == "--config") (-1) caddyArgs
  );

  assertions = [
    {
      assertion = disabled.launchd.daemons == {};
      message = "the module must stay inert until services.localhostProxy.enable is set";
    }
    {
      assertion = lib.all (arg: lib.elem arg dnsmasqArgs) [
        "--listen-address=127.0.0.1"
        "--listen-address=::1"
        "--server=100.100.100.100"
        "--local=/localhost/"
        "--address=/localhost/127.0.0.1"
        "--address=/localhost/::1"
      ];
      message = "dnsmasq must listen dual-stack, forward to MagicDNS, and keep every *.localhost RR local";
    }
    {
      assertion = !lib.elem "--domain-needed" dnsmasqArgs;
      message = "dnsmasq must forward single-label MagicDNS names";
    }
    {
      assertion = lib.isString caddyfile && lib.hasPrefix builtins.storeDir caddyfile;
      message = "caddy must run from a store-rendered Caddyfile";
    }
    {
      assertion = enabled.launchd.daemons.caddy.serviceConfig.EnvironmentVariables.HOME == "/var/root";
      message = "caddy must get a HOME (launchd daemons have none and Caddy refuses to start without one)";
    }
    {
      assertion = dnsmasqEntrypoint == lib.getExe pkgs.dnsmasq;
      message = "setWifiDns must be opt-in";
    }
    {
      assertion = lib.isString wifiDnsmasqEntrypoint && wifiDnsmasqEntrypoint != dnsmasqEntrypoint;
      message = "setWifiDns must run dnsmasq through the Wi-Fi DNS lifecycle owner";
    }
  ];

  failures = map (a: a.message) (lib.filter (a: !a.assertion) assertions);
in
  assert lib.assertMsg (failures == []) (
    "localhost-proxy:\n  " + lib.concatStringsSep "\n  " failures
  );
    pkgs.runCommand "ix-test-localhost-proxy" {
      __structuredAttrs = true;
      strictDeps = true;
      env.caddyfile = caddyfile;
      env.wifiDnsmasqEntrypoint = wifiDnsmasqEntrypoint;
    } ''
      grep -F '/usr/sbin/networksetup -setdnsservers Wi-Fi 127.0.0.1 ::1' "$wifiDnsmasqEntrypoint"
      grep -F '/usr/sbin/networksetup -setdnsservers Wi-Fi Empty' "$wifiDnsmasqEntrypoint"
      grep -F 'trap restore_dns EXIT' "$wifiDnsmasqEntrypoint"
      grep -F 'trap terminate HUP INT TERM' "$wifiDnsmasqEntrypoint"
      grep -F 'kill -TERM "$dnsmasq_pid"' "$wifiDnsmasqEntrypoint"
      mkdir -p "$out"
    ''
