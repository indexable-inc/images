# Caddy-based reverse-proxy gateway. Stock starting point for "expose only
# some ports" workflows under the primitives-only networking rule (see
# `ix/AGENTS.md` and the `VM networking` section of `AGENTS.md`): users put a
# gateway VM in front of their backend VMs, declare `services.gateway.routes`
# in Nix, and ix's host layer never sees per-port intent.
{
  config,
  lib,
  ...
}:
let
  inherit (lib)
    mkEnableOption
    mkIf
    mkOption
    types
    ;
  cfg = config.services.gateway;
in
{
  options.services.gateway = {
    enable = mkEnableOption "Caddy reverse-proxy gateway";

    routes = mkOption {
      type = types.attrsOf types.str;
      default = { };
      example = {
        "example.com" = "http://backend:8080";
      };
      description = ''
        Map from Caddy site address (typically a public hostname, optionally
        with a path matcher) to upstream URL. Each entry becomes a Caddy
        virtual host with a `reverse_proxy` directive. Set at least one entry
        before deploying or Caddy will start with no listeners.
      '';
    };

    tlsEmail = mkOption {
      type = types.nullOr types.str;
      default = null;
      example = "ops@example.com";
      description = ''
        Contact email for Let's Encrypt account registration. Required for
        automatic HTTPS on real domains; leave null for local-only deployments
        where Caddy will issue an internal CA cert instead.
      '';
    };

    httpRedirect = mkOption {
      type = types.bool;
      default = true;
      description = ''
        Redirect HTTP traffic to HTTPS. Off only for intranet routes that must
        serve plain HTTP (Caddy still issues internal certs by default).
      '';
    };

    maxRequestBodyMiB = mkOption {
      type = types.ints.positive;
      default = 32;
      description = "Maximum request body size, in mebibytes. Applied to every route.";
    };
  };

  config = mkIf cfg.enable {
    services.caddy = {
      enable = true;
      email = mkIf (cfg.tlsEmail != null) cfg.tlsEmail;
      globalConfig = mkIf (!cfg.httpRedirect) ''
        auto_https disable_redirects
      '';
      virtualHosts = lib.mapAttrs (_host: upstream: {
        extraConfig = ''
          reverse_proxy ${upstream}
          request_body {
            max_size ${toString cfg.maxRequestBodyMiB}MB
          }
          encode gzip zstd
          log {
            output stdout
            format json
          }
        '';
      }) cfg.routes;
    };

    networking.firewall.allowedTCPPorts = [
      80
      443
    ];
  };
}
