# ix-term: tailnet-internal web terminal (index#3797).
#
# Runs the ix-term server as a long-lived systemd service: server-side
# libghostty-vt terminal state per session, PTY login shells spawned as
# {option}`user`, and the browser UI served from the packaged Svelte site.
# The `ixterm` CLI in a session resolves its pts through
# `/run/ix-term/sessions/<id>/pts` (the service's runtime directory).
#
# Auth: the server binds {option}`listenAddress` (loopback by default) and
# trusts whatever sits in front of it. The intended deployment terminates
# term.ix.dev on the tailnet with a reverse proxy on the same host; per-request
# Tailscale WhoIs identity is a TODO tracked in index#3797. There is no login
# UI by design, so never point this at a non-loopback address without a
# tailnet-scoped proxy in front. Host wiring (term.ix.dev DNS, the proxy, and
# which fleet host serves it) lands in the ix repo, not here.
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
    mkPackageOption
    types
    ;
  cfg = config.services.ix-term;
in {
  options.services.ix-term = {
    enable = mkEnableOption "the ix-term web terminal server";

    package = mkPackageOption pkgs "ix-term-server" {};

    listenAddress = mkOption {
      type = types.str;
      default = "127.0.0.1";
      description = ''
        Address the server binds. Keep it loopback and let the tailnet
        reverse proxy terminate term.ix.dev; the server itself performs no
        authentication.
      '';
    };

    port = mkOption {
      type = types.port;
      default = 7533;
      description = "TCP port for the UI, the REST API, and the websockets.";
    };

    user = mkOption {
      type = types.str;
      default = "ix-term";
      description = ''
        User the server (and therefore every session's login shell) runs as.
        The default system user is created with a bash login shell and a home
        under /var/lib/ix-term; point this at a real account instead when
        sessions should be that person's environment.
      '';
    };

    scrollback = mkOption {
      type = types.ints.positive;
      default = 10000;
      description = "Scrollback lines kept per session by the VT engine.";
    };
  };

  config = mkIf cfg.enable {
    users.users.ix-term = mkIf (cfg.user == "ix-term") {
      isSystemUser = true;
      group = "ix-term";
      # Sessions are login shells; the default user needs a real shell and a
      # writable home for one to start in.
      shell = pkgs.bashInteractive;
      home = "/var/lib/ix-term";
      createHome = true;
    };
    users.groups.ix-term = mkIf (cfg.user == "ix-term") {};

    ix.networking.portClaims.ix-term = {
      protocol = "tcp";
      inherit (cfg) port;
      address = cfg.listenAddress;
      description = "ix-term web terminal";
    };

    systemd.services.ix-term = {
      description = "ix-term web terminal server";
      wantedBy = ["multi-user.target"];
      after = ["network.target"];
      serviceConfig = {
        Type = "simple";
        User = cfg.user;
        # /run/ix-term is the pts-mapping contract with the `ixterm` CLI, so
        # the service owns it for its whole life (no DynamicUser: the path and
        # the session shells must belong to a stable account).
        RuntimeDirectory = "ix-term";
        RuntimeDirectoryMode = "0755";
        ExecStart = lib.escapeShellArgs [
          (lib.getExe cfg.package)
          "--listen"
          "${cfg.listenAddress}:${toString cfg.port}"
          "--scrollback"
          (toString cfg.scrollback)
        ];
        Restart = "on-failure";
        RestartSec = 2;
        # Deliberately no ix.systemdHardening: sessions are real login shells
        # for cfg.user, so ProtectHome/ProtectSystem would gut the product.
      };
      environment.IX_TERM_RUNTIME_DIR = "/run/ix-term";
    };
  };
}
