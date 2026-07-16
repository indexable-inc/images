# Loopback service naming (macOS): stable names (http://nwm.localhost)
# for loopback services instead of memorized ports. dnsmasq on loopback :53
# answers *.localhost with loopback addresses (RFC 6761 reserves .localhost
# for exactly this) and Caddy on loopback :80 routes by Host header. Adding
# a service is one line in `services.localhostProxy.services`.
{writeBashApplication}: {
  config,
  lib,
  pkgs,
  ...
}: let
  cfg = config.services.localhostProxy;
  dnsmasq = lib.getExe pkgs.dnsmasq;

  # Keep the persistent networksetup mutation inside the launchd job that
  # needs it. launchd sends SIGTERM when unloading a job, so disabling the
  # option or removing the module restores automatic DNS as dnsmasq stops.
  dnsmasqWithWifiDns = writeBashApplication pkgs {
    name = "dnsmasq-with-wifi-dns";
    runtimeInputs = [pkgs.dnsmasq];
    text = ''
      restore_dns() {
        /usr/sbin/networksetup -setdnsservers Wi-Fi Empty
      }

      terminate() {
        if [ -n "$dnsmasq_pid" ]; then
          kill -TERM "$dnsmasq_pid"
          wait "$dnsmasq_pid"
        fi
        exit 0
      }

      dnsmasq_pid=
      trap restore_dns EXIT
      trap terminate HUP INT TERM
      /usr/sbin/networksetup -setdnsservers Wi-Fi 127.0.0.1 ::1
      dnsmasq "$@" &
      dnsmasq_pid=$!
      wait "$dnsmasq_pid"
    '';
  };
  dnsmasqWithWifiDnsExe = lib.getExe dnsmasqWithWifiDns;

  dnsmasqArgs = [
    "--keep-in-foreground"
    "--listen-address=127.0.0.1"
    "--listen-address=::1"
    "--port=53"
    "--no-resolv"
    "--bogus-priv"
    "--cache-size=1000"
    "--server=100.100.100.100"
    # Keep every RR type for *.localhost inside the local resolver. Since
    # dnsmasq 2.86, --address alone forwards non-A/AAAA queries upstream:
    # https://thekelleys.org.uk/dnsmasq/docs/dnsmasq-man.html
    "--local=/localhost/"
    "--address=/localhost/127.0.0.1"
    "--address=/localhost/::1"
  ];

  # Rendered and parse-checked at build time so a bad Caddyfile fails the
  # switch, not the running daemon. http:// + auto_https off keep Caddy from
  # minting TLS certs (no keychain trust prompts); default_bind keeps every
  # listener loopback-only so nothing proxied leaks onto the LAN.
  caddyfile = let
    site = name: port: ''
      http://${name}.localhost {
        reverse_proxy 127.0.0.1:${toString port}
      }
    '';
    rendered = pkgs.writeText "Caddyfile.in" ''
      {
        admin off
        auto_https off
        default_bind 127.0.0.1 ::1
      }
      ${lib.concatStrings (lib.mapAttrsToList site cfg.services)}
    '';
  in
    pkgs.runCommandLocal "Caddyfile" {
      strictDeps = true;
      nativeBuildInputs = [pkgs.caddy];
    } ''
      HOME=$TMPDIR caddy validate --config ${rendered} --adapter caddyfile
      cp ${rendered} $out
    '';
in {
  options.services.localhostProxy = {
    enable = lib.mkEnableOption "the loopback localhost proxy (dnsmasq wildcard DNS + Caddy vhost registry)";

    services = lib.mkOption {
      type = lib.types.attrsOf lib.types.port;
      default = {};
      example = lib.literalExpression ''
        {
          nwm = 7532;
          weave = 7677;
        }
      '';
      description = ''
        Loopback reverse proxy registry: `http://<name>.localhost` ->
        `127.0.0.1:<port>`.
      '';
    };

    setWifiDns = lib.mkOption {
      type = lib.types.bool;
      default = false;
      description = ''
        Point Wi-Fi DNS at the local dnsmasq (127.0.0.1, ::1) while its
        launchd job is running. Unloading the job restores automatic DNS.
        Only safe AFTER setting global nameservers in the Tailscale admin
        panel (DNS tab): otherwise Tailscale's MagicDNS forwards non-tailnet
        queries through the system resolver, which sends them back to
        MagicDNS in a loop.

        Prerequisite: Tailscale admin -> DNS -> add global nameservers
        (e.g. 1.1.1.1, 8.8.8.8) and enable "Override local DNS". This makes
        MagicDNS use the tunnel for upstream resolution, breaking the cycle.
      '';
    };
  };

  config = lib.mkIf cfg.enable {
    # Local DNS resolver: dnsmasq on loopback, dual-stack.
    #
    # macOS's mDNSResponder wedges periodically on IPv4-only interfaces:
    # en0 has no IPv6 default route, so resolver #2 gets "Request A records"
    # only. AAAA queries fall through to mDNS resolvers with 5 s timeouts.
    # When mDNSResponder's state gets stale (sleep/wake, VPN flap), every
    # cold getaddrinfo pays a flat 5 s penalty.
    #
    # Running dnsmasq on 127.0.0.1 + ::1 and pointing Wi-Fi DNS there gives
    # macOS both A and AAAA resolver flags. dnsmasq handles both query types,
    # forwards everything to Tailscale's MagicDNS (100.100.100.100), which
    # answers tailnet names itself and forwards the rest to the global
    # nameservers configured in the Tailscale admin panel (see setWifiDns).
    launchd.daemons.dnsmasq = {
      serviceConfig = {
        Label = "org.nixos.dnsmasq";
        ProgramArguments =
          [
            (
              if cfg.setWifiDns
              then dnsmasqWithWifiDnsExe
              else dnsmasq
            )
          ]
          ++ dnsmasqArgs;
        KeepAlive = true;
        RunAtLoad = true;
        StandardErrorPath = "/var/log/dnsmasq.log";
      };
    };

    # Host-header reverse proxy for the cfg.services registry. Root daemon so
    # it can bind :80; sockets stay loopback-only via default_bind.
    launchd.daemons.caddy = {
      serviceConfig = {
        Label = "org.nixos.caddy";
        ProgramArguments = [
          (lib.getExe pkgs.caddy)
          "run"
          "--config"
          "${caddyfile}"
          "--adapter"
          "caddyfile"
        ];
        # launchd daemons get no HOME; Caddy refuses to start without one for
        # its storage dir, even with TLS off. Root's home always exists.
        EnvironmentVariables.HOME = "/var/root";
        KeepAlive = true;
        RunAtLoad = true;
        StandardErrorPath = "/var/log/caddy.log";
      };
    };
  };
}
