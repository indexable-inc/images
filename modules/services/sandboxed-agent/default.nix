/**
Run a confined agent: an interactive AI coding agent (or any command) that
can do real work on the machine but can neither read its own API credential
nor reach the network beyond one loopback proxy.

Three properties, each with one owner:

1. The agent can do real work. It runs as an unprivileged fixed-uid user in
   the ordinary mount namespace: /nix/store and the system toolchain
   read-only, its own home and /tmp read-write. No bwrap/chroot layer, so
   the environment it sees is the one a human shell gets.
2. The agent can never read the credential. The key file is owned by the
   proxy's own user (attach it with mode 0400 through the deployment's
   secret machinery); only the loopback proxy (./proxy, a small Rust
   binary) reads it, per request, and injects it upstream. The agent is
   handed whatever decoy environment the caller configures.
3. The agent can never reach the internet. An nftables output-hook policy
   keyed on the agent's uid rejects every packet except TCP to the proxy's
   127.0.0.1 port. The uid is the boundary, so it covers every descendant
   process, and the wrapper pins the uid with NoNewPrivileges. DNS is
   irrelevant to the guarantee: the agent talks to a literal loopback
   address, and any resolver traffic from its uid is rejected like
   everything else.

The consumer surface is intent-sized: name the confined user and uid, point
`command.program` at the real binary, and describe the proxy's one upstream
(host, credential header, key file). See examples/kernel-build for
the pattern in use.
*/
{
  config,
  ix,
  lib,
  pkgs,
  ...
}: let
  inherit (lib) mkEnableOption mkIf mkOption types;

  cfg = config.services.sandboxed-agent;

  systemdRun = "${config.systemd.package}/bin/systemd-run";

  egressTable = "${cfg.user}-egress";

  # Per-unit content-addressed builds out of the shared repo workspace graph
  # (lib/rust/workspace.nix); sources live in ./proxy, ./launch, and
  # ./egress-check.
  proxy = ix.cargoUnit.selectBinaryWithTests ix.rustWorkspace.units {
    binary = "sandboxed-agent-proxy";
    meta.mainProgram = "sandboxed-agent-proxy";
  };

  launch = ix.cargoUnit.selectBinaryWithTests ix.rustWorkspace.units {
    binary = "sandboxed-agent-launch";
    meta.mainProgram = "sandboxed-agent-launch";
  };

  egressCheck = ix.cargoUnit.selectBinaryWithTests ix.rustWorkspace.units {
    binary = "sandboxed-agent-egress-check";
    meta.mainProgram = "sandboxed-agent-egress-check";
  };

  proxyArgs = [
    "--port"
    (toString cfg.proxy.port)
    "--key-file"
    cfg.proxy.secretFile
    "--upstream"
    cfg.proxy.upstream
    "--header"
    cfg.proxy.header
  ];

  sessionEnvironment =
    {
      HOME = "/home/${cfg.user}";
      # Transient units start from systemd's compiled-in default PATH, which
      # points nowhere on NixOS; hand the agent the system profile.
      PATH = "/run/current-system/sw/bin:/run/wrappers/bin";
    }
    // cfg.environment;

  # The operator's entry point, installed on PATH as `cfg.command.name`; the
  # real binary stays off PATH. The compiled launcher (./launch) checks it
  # was started as root, drops into the unprivileged agent user via
  # systemd-run, and execs `cfg.command.program` with the session
  # environment. Its configuration is baked in as env vars through a binary
  # wrapper -- the config-launch idiom -- so no shell enters the sandbox
  # boundary.
  wrapper =
    pkgs.runCommand cfg.command.name {
      nativeBuildInputs = [pkgs.makeBinaryWrapper];
      meta.mainProgram = cfg.command.name;
    } ''
      makeBinaryWrapper ${lib.getExe launch} $out/bin/${cfg.command.name} \
        --set SANDBOXED_AGENT_SYSTEMD_RUN ${lib.escapeShellArg systemdRun} \
        --set SANDBOXED_AGENT_USER ${lib.escapeShellArg cfg.user} \
        --set SANDBOXED_AGENT_PROGRAM ${lib.escapeShellArg cfg.command.program} \
        ${lib.concatStringsSep " \\\n        " (
        lib.mapAttrsToList (
          name: value: "--set ${lib.escapeShellArg "SANDBOXED_AGENT_SETENV_${name}"} ${lib.escapeShellArg value}"
        )
        sessionEnvironment
      )}
    '';

  egressCheckArgs = [
    "--nft"
    (lib.getExe' pkgs.nftables "nft")
    "--systemd-run"
    systemdRun
    "--user"
    cfg.user
    "--table"
    egressTable
    "--proxy-port"
    (toString cfg.proxy.port)
    "--upstream"
    cfg.proxy.upstream
  ];
in {
  options.services.sandboxed-agent = {
    enable = mkEnableOption "a confined agent whose only network reach is a key-injecting loopback proxy";

    user = mkOption {
      type = types.str;
      default = "agent";
      description = "Name of the confined user (and its primary group) the agent runs as.";
    };

    uid = mkOption {
      type = types.ints.positive;
      example = 1100;
      description = ''
        Fixed uid for the confined user. Pinned so the nftables rules match
        a literal number instead of depending on name resolution at ruleset
        load; also reused as the group's gid.
      '';
    };

    command = {
      name = mkOption {
        type = types.str;
        default = cfg.user;
        defaultText = lib.literalExpression "config.services.sandboxed-agent.user";
        description = "Name the confining wrapper is installed under on PATH.";
      };

      program = mkOption {
        type = types.path;
        example = lib.literalExpression "lib.getExe pkgs.claude-code";
        description = ''
          The real agent executable. It stays off PATH; the wrapper is the
          only door, and it enters the confined uid first.
        '';
      };
    };

    environment = mkOption {
      type = types.attrsOf types.str;
      default = {};
      description = ''
        Extra environment for the agent's session, on top of the
        module-managed HOME, PATH, and TERM. This is where the agent's
        base-URL redirection and decoy credential belong (for an Anthropic
        agent: ANTHROPIC_BASE_URL pointing at `proxy.url` and a dummy
        ANTHROPIC_API_KEY the proxy strips).
      '';
    };

    proxy = {
      port = mkOption {
        type = types.port;
        example = 8402;
        description = "Loopback TCP port the key-injecting proxy listens on.";
      };

      upstream = mkOption {
        type = types.str;
        example = "api.anthropic.com";
        description = "The single HTTPS host the proxy forwards to; the proxy's entire upstream world.";
      };

      header = mkOption {
        type = types.str;
        example = "x-api-key";
        description = "Request header the proxy overwrites with the real credential.";
      };

      secretFile = mkOption {
        type = types.str;
        example = "/run/secrets/anthropic/api-key";
        description = ''
          Runtime path of the credential file. Attach it owned by
          `proxy.user` with mode 0400 (a deployment secret declaration, not
          a build input), so the agent's uid cannot pass the DAC check. The
          proxy reads it per request, so rotation needs no restart.
        '';
      };

      user = mkOption {
        type = types.str;
        default = "${cfg.user}-proxy";
        defaultText = lib.literalExpression ''"''${config.services.sandboxed-agent.user}-proxy"'';
        description = "System user that owns the credential file and runs the proxy.";
      };

      url = mkOption {
        type = types.str;
        readOnly = true;
        default = "http://127.0.0.1:${toString cfg.proxy.port}";
        defaultText = lib.literalExpression ''"http://127.0.0.1:''${port}"'';
        description = "Read-only convenience: the proxy's base URL, for wiring into `environment`.";
      };
    };
  };

  config = mkIf cfg.enable {
    users = {
      groups = {
        ${cfg.user}.gid = cfg.uid;
        ${cfg.proxy.user} = {};
      };
      users = {
        ${cfg.user} = {
          isNormalUser = true;
          inherit (cfg) uid;
          group = cfg.user;
          description = "Sandboxed agent";
        };
        ${cfg.proxy.user} = {
          isSystemUser = true;
          group = cfg.proxy.user;
          description = "Owns the ${cfg.proxy.upstream} credential and the loopback proxy";
        };
      };
    };

    environment.systemPackages = [wrapper];

    systemd.services.sandboxed-agent-proxy = {
      description = "Key-injecting loopback proxy to ${cfg.proxy.upstream}";
      # No network-online ordering: the listener is loopback and the upstream
      # connection happens lazily per request.
      wantedBy = ["multi-user.target"];
      serviceConfig =
        ix.systemdHardening
        // {
          User = cfg.proxy.user;
          Group = cfg.proxy.user;
          ExecStart = lib.escapeShellArgs ([(lib.getExe proxy)] ++ proxyArgs);
          Restart = "on-failure";
          RestartSec = "2s";
        };
    };

    # The network boundary. An output-hook filter keyed on the socket's uid
    # covers every process the agent ever spawns (uid is inherited and, with
    # NoNewPrivileges, unchangeable). reject rather than drop so a hostile or
    # merely curious process fails fast instead of hanging on timeouts.
    # Loopback is inside the deny: the agent cannot probe other local
    # listeners (ix-console, the guest agent), only the proxy port.
    networking.nftables.tables.${egressTable} = {
      family = "inet";
      content = ''
        chain output {
          type filter hook output priority filter; policy accept;

          meta skuid ${toString cfg.uid} ip daddr 127.0.0.1 tcp dport ${toString cfg.proxy.port} accept
          meta skuid ${toString cfg.uid} meta l4proto tcp reject with tcp reset
          meta skuid ${toString cfg.uid} counter reject
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
      networking.portClaims.sandboxed-agent-proxy = {
        protocol = "tcp";
        port = cfg.proxy.port;
        address = "127.0.0.1";
        description = "Key-injecting loopback proxy for the sandboxed agent";
      };

      healthChecks = {
        sandboxed-agent-proxy = {
          description = "key-injecting loopback proxy is listening";
          tcp = {port = cfg.proxy.port;};
        };
        sandboxed-agent-egress = {
          description = "the ${cfg.user} uid reaches the proxy and nothing else";
          attempts = 3;
          command = [(lib.getExe egressCheck)] ++ egressCheckArgs;
        };
      };
    };
  };
}
