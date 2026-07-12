# Eval test for the darwin loopback localhost proxy module (#2988).
# Evaluates modules/darwin/localhost-proxy.nix against stub nix-darwin
# options and asserts the wiring: dnsmasq answers *.localhost on loopback and
# forwards upstream to MagicDNS, Caddy serves the vhost registry from a
# build-time-validated Caddyfile, and the Wi-Fi DNS hook lands in
# postActivation only when opted in. The returned check derivation depends on
# the rendered Caddyfile, so `caddy validate` genuinely gates CI.
{
  lib,
  pkgs,
  paths,
}: let
  launchdDaemonType = lib.types.submodule {
    options.serviceConfig = lib.mkOption {
      type = lib.types.attrsOf lib.types.raw;
      default = {};
    };
  };

  activationScriptType = lib.types.submodule {
    options.text = lib.mkOption {
      type = lib.types.lines;
      default = "";
    };
  };

  optionStubs = {
    options = {
      launchd.daemons = lib.mkOption {
        type = lib.types.attrsOf launchdDaemonType;
        default = {};
      };
      system.activationScripts = lib.mkOption {
        type = lib.types.attrsOf activationScriptType;
        default = {};
      };
    };
  };

  eval = extraModule:
    (lib.evalModules {
      modules = [
        optionStubs
        (paths.root + "/modules/darwin/localhost-proxy.nix")
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
  caddyArgs = enabled.launchd.daemons.caddy.serviceConfig.ProgramArguments;
  # The element after `--config`: the validated Caddyfile store path. Built
  # by the check below so a Caddyfile `caddy validate` rejects fails CI.
  caddyfile = builtins.elemAt caddyArgs (
    1 + lib.lists.findFirstIndex (arg: arg == "--config") (-1) caddyArgs
  );

  postActivationOf = config: (config.system.activationScripts.postActivation or {text = "";}).text;
  setsWifiDns = config: lib.hasInfix "networksetup -setdnsservers Wi-Fi 127.0.0.1 ::1" (postActivationOf config);

  assertions = [
    {
      assertion = disabled.launchd.daemons == {} && disabled.system.activationScripts == {};
      message = "the module must stay inert until services.localhostProxy.enable is set";
    }
    {
      assertion = lib.all (arg: lib.elem arg dnsmasqArgs) [
        "--listen-address=127.0.0.1"
        "--listen-address=::1"
        "--server=100.100.100.100"
        "--address=/localhost/127.0.0.1"
        "--address=/localhost/::1"
      ];
      message = "dnsmasq must listen dual-stack on loopback, forward to MagicDNS, and wildcard *.localhost";
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
      assertion = !setsWifiDns enabled;
      message = "setWifiDns must be opt-in: it is only safe with Tailscale admin global nameservers set";
    }
    {
      assertion = setsWifiDns wifiDns;
      message = "setWifiDns must point Wi-Fi DNS at the loopback dnsmasq via postActivation";
    }
  ];

  failures = map (a: a.message) (lib.filter (a: !a.assertion) assertions);
in
  assert lib.assertMsg (failures == []) (
    "localhost-proxy:\n  " + lib.concatStringsSep "\n  " failures
  );
    pkgs.runCommand "ix-test-localhost-proxy" {
      __structuredAttrs = true;
      env.caddyfile = caddyfile;
    } ''
      mkdir -p "$out"
    ''
