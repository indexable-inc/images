/**
A key-blind, offline Claude Code agent that can still build kernels.

Three properties, each with one owner:

1. Claude can do real work. It runs as the unprivileged `claude` user in
   the ordinary mount namespace: /nix/store and the toolchain read-only,
   the kernel tree (chowned to it after the clone), its own ~/.claude and
   /tmp read-write. No bwrap/chroot layer to keep a working kbuild host.
2. Claude can never read the API key. The key file is owned by the
   `anthropic-proxy` user, mode 0400 (declared in default.ix); only the
   loopback proxy (anthropic-proxy.py) reads it, per request, and injects
   it upstream. The agent gets a dummy ANTHROPIC_API_KEY and a loopback
   ANTHROPIC_BASE_URL.
3. Claude can never reach the internet. An nftables output-hook policy
   keyed on the `claude` uid rejects every packet except TCP to the proxy's
   127.0.0.1 port. The uid is the boundary, so it covers every descendant
   process, and the `claude` wrapper pins the uid with NoNewPrivileges.
   DNS is irrelevant to the guarantee: the agent talks to a literal
   loopback address, and any resolver traffic from its uid is rejected
   like everything else.
*/
{
  config,
  ix,
  lib,
  pkgs,
  ...
}: let
  # Pinned so the nftables rules below match a literal number instead of
  # depending on name resolution at ruleset load.
  claudeUid = 1100;

  # Loopback-only. Claimed in ix.networking.portClaims so an eval-time error
  # (not a runtime bind race) catches another module wanting the port.
  proxyPort = 8402;

  # Where default.ix's secret declaration materializes the key: owner
  # anthropic-proxy, mode 0400. The `claude` uid cannot pass the DAC check.
  keyFile = "/run/secrets/anthropic/api-key";

  # Single source of truth for the checkout path is the git-clone declaration
  # in default.ix.
  srcDir = config.services.git-clone.dest;

  systemdRun = "${config.systemd.package}/bin/systemd-run";

  proxy = ix.writePythonApplication pkgs {
    name = "anthropic-proxy";
    src = ./anthropic-proxy.py;
    args = [
      (toString proxyPort)
      keyFile
    ];
  };

  # Interactive entry point, installed as `claude`; the real claude-code
  # binary stays off PATH. See claude-wrapper.py for the mechanism.
  claudeWrapper = ix.writePythonApplication pkgs {
    name = "claude";
    src = ./claude-wrapper.py;
    args = [
      systemdRun
      "${pkgs.claude-code}/bin/claude"
      "http://127.0.0.1:${toString proxyPort}"
      # Transient units start from systemd's compiled-in default PATH, which
      # points nowhere on NixOS; hand the agent the system profile.
      "/run/current-system/sw/bin:/run/wrappers/bin"
      config.environment.variables.CPATH
      config.environment.variables.LIBRARY_PATH
      config.environment.variables.PKG_CONFIG_PATH
    ];
  };

  egressCheck = ix.writePythonApplication pkgs {
    name = "check-claude-egress";
    src = ./check-claude-egress.py;
    args = [
      systemdRun
      (lib.getExe' pkgs.nftables "nft")
      (lib.getExe' pkgs.netcat-openbsd "nc")
      (lib.getExe pkgs.curl)
      (toString proxyPort)
    ];
  };
in {
  users = {
    groups = {
      claude.gid = claudeUid;
      anthropic-proxy = {};
    };
    users = {
      claude = {
        isNormalUser = true;
        uid = claudeUid;
        group = "claude";
        description = "Sandboxed kernel-hacking agent";
      };
      anthropic-proxy = {
        isSystemUser = true;
        group = "anthropic-proxy";
        description = "Owns the Anthropic API key and the loopback proxy";
      };
    };
  };

  environment.systemPackages = [claudeWrapper];

  systemd.services = {
    anthropic-proxy = {
      description = "Key-injecting loopback proxy to api.anthropic.com";
      # No network-online ordering: the listener is loopback and the upstream
      # connection happens lazily per request.
      wantedBy = ["multi-user.target"];
      serviceConfig =
        ix.systemdHardening
        // {
          User = "anthropic-proxy";
          Group = "anthropic-proxy";
          ExecStart = lib.getExe proxy;
          Restart = "on-failure";
          RestartSec = "2s";
        };
    };

    # The clone runs as root (it needs the network the agent is denied); hand
    # the tree to the agent afterwards. Re-runs alongside the idempotent
    # clone on every boot; a re-chown of an existing tree is a few seconds.
    git-clone = lib.mkIf config.services.git-clone.enable {
      serviceConfig.ExecStartPost = "${pkgs.coreutils}/bin/chown -R claude:claude ${srcDir}";
    };
  };

  # The network boundary. An output-hook filter keyed on the socket's uid
  # covers every process the agent ever spawns (uid is inherited and, with
  # NoNewPrivileges, unchangeable). reject rather than drop so a hostile or
  # merely curious process fails fast instead of hanging on timeouts.
  # Loopback is inside the deny: the agent cannot probe other local
  # listeners (ix-console, the guest agent), only the proxy port.
  networking.nftables.tables.claude-egress = {
    family = "inet";
    content = ''
      chain output {
        type filter hook output priority filter; policy accept;

        meta skuid ${toString claudeUid} ip daddr 127.0.0.1 tcp dport ${toString proxyPort} accept
        meta skuid ${toString claudeUid} meta l4proto tcp reject with tcp reset
        meta skuid ${toString claudeUid} counter reject
      }
    '';
  };

  # The nix daemon would otherwise be an indirect egress path: any local uid
  # may ask it to realise a fixed-output derivation, and the daemon (root,
  # outside the nftables policy) fetches the URL for it. Scope the socket to
  # operators so the agent's only network reach really is the proxy.
  nix.settings.allowed-users = [
    "root"
    "@wheel"
  ];

  ix = {
    networking.portClaims.anthropic-proxy = {
      protocol = "tcp";
      port = proxyPort;
      address = "127.0.0.1";
      description = "Anthropic key-injecting loopback proxy";
    };

    healthChecks = {
      anthropic-proxy = {
        description = "key-injecting loopback proxy is listening";
        tcp = {port = proxyPort;};
      };
      claude-egress = {
        description = "claude's uid reaches the proxy and nothing else";
        attempts = 3;
        command = [(lib.getExe egressCheck)];
      };
    };
  };
}
