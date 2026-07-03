# One Ray cluster spanning the tailnet, plus the ix-mcp engine that drives it.
#
# Deployment side of the `fleet` Python module bundled in ix-mcp:
# `fleet.run`/`fleet.submit` ship cloudpickled callables to Ray and the Ray
# object store (Plasma) carries the data; `fleet.in_kernel` runs code in a
# node's live ix-mcp session over the `/api/exec` endpoint.
#
# Topology: ONE Ray cluster. Exactly one node sets `role = "head"` (it holds the
# GCS); the rest are `role = "worker"` pointing `headAddress` at the head's
# tailscale IP. Daemons bind their *tailscale* IPv4 (runtime host state), so the
# cluster lives on the tailnet, which is also the trust boundary (Ray has no
# per-call auth of its own).
#
# Repo-agnostic on purpose: this module declares no `ix.*` NixOS *options*
# (port-claim/health-check bookkeeping), so it imports cleanly into any NixOS
# system -- the index fleet, the ix monorepo hosts, a personal box -- without
# dragging in either repo's option tree. It needs only the index flake lib
# (`writeNushellApplication`/`systemdHardening`), taken via the `indexLib` arg
# (see its note below), and the ix-mcp engine, handed in via
# {option}`services.ix-ray.notebookPackage`.
#
# The node-level correctness here (pinned inter-node ports, a SHORT `/run/ray`
# temp-dir so Ray's AF_UNIX plasma socket stays under the 108-byte sun_path
# limit, and PrivateDevices/PrivateUsers off so an attaching kernel can map the
# shared-memory object store) mirrors the proven `examples/ray/cluster`
# cluster-node module. Use nixpkgs `python3Packages.ray` -- the same Ray the
# ix-mcp interpreter imports, so the cluster and the kernels driving it run an
# identical version (Ray requires matching versions cluster-wide) and the
# daemons are FHS-correct with no nix-ld shim.
# `indexLib` is the index flake lib (writeNushellApplication/systemdHardening),
# supplied by the consumer via `_module.args.indexLib`. It is deliberately NOT
# named `ix`: a host binds `ix` to its own specialArg (the ix monorepo's is a
# different shape entirely), and a module formal cannot fall back to that, so a
# distinct name is the only collision-free way to take the index lib. In index's
# own eval contexts wire `_module.args.indexLib = ix`; elsewhere
# `_module.args.indexLib = inputs.index.lib`.
{
  config,
  indexLib,
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
    optional
    optionalAttrs
    optionals
    types
    ;

  cfg = config.services.ix-ray;

  # Spill target lives on real disk (the StateDirectory), NOT under the tmpfs
  # `/run/ray` temp-dir -- otherwise "spill to disk under memory pressure" would
  # just consume RAM. directory_path is created by Ray under the StateDirectory.
  spillDir = "/var/lib/ray/spill";
  spillConfig = builtins.toJSON {
    type = "filesystem";
    params.directory_path = spillDir;
  };

  ray = lib.getExe' cfg.package "ray";

  # Mode-specific `ray start` flags. Common flags (pinned ports, temp-dir, spill,
  # --node-ip-address, --block) are appended in the launcher once the tailscale
  # IP is resolved. Ray's web dashboard needs the `ray[default]` extras nixpkgs
  # core ray omits, so it is disabled; cluster state comes from `fleet.nodes()`.
  # The head also pins the Ray Client server port so off-cluster `ray://` clients
  # (e.g. a laptop driving the fleet via `fleet.connect()`) reach a known port.
  modeArgs =
    if cfg.role == "head"
    then [
      "--head"
      "--port"
      (toString cfg.gcsPort)
      "--ray-client-server-port"
      (toString cfg.clientServerPort)
      "--include-dashboard"
      "false"
    ]
    else [
      "--address"
      "${cfg.headAddress}:${toString cfg.gcsPort}"
    ];

  commonArgs =
    [
      "--node-manager-port"
      (toString cfg.nodeManagerPort)
      "--object-manager-port"
      (toString cfg.objectManagerPort)
      "--min-worker-port"
      (toString cfg.workerPortLow)
      "--max-worker-port"
      (toString cfg.workerPortHigh)
      "--temp-dir"
      "/run/ray"
      "--object-spilling-config"
      spillConfig
    ]
    ++ optionals (cfg.objectStoreMemory != null) [
      "--object-store-memory"
      (toString cfg.objectStoreMemory)
    ];

  startArgsNu = "[ ${lib.concatMapStringsSep " " builtins.toJSON (modeArgs ++ commonArgs)} ]";

  # Resolve this node's tailscale IPv4 at runtime (it is host state, not a Nix
  # value), fail loudly if tailscale is not up, then exec `ray start` bound to
  # it. `--block` keeps the daemon in the foreground for systemd Type=simple.
  rayLauncher = indexLib.writeNushellApplication pkgs {
    name = "ix-ray-launch";
    meta.description = "Resolve this node's tailscale IPv4 and exec the ray daemon bound to it";
    runtimeInputs = [
      pkgs.tailscale
      cfg.package
    ];
    text = ''
      # nu
      def main [] {
        let ip = (do --ignore-errors {
          ^tailscale ip -4 | lines | where ($it | str trim | is-not-empty) | first
        } | default "")
        if ($ip | str trim | is-empty) {
          print --stderr "ix-ray: no tailscale IPv4 yet; is tailscaled up?"
          exit 1
        }
        let args = [ ...${startArgsNu} "--node-ip-address" $ip "--block" ]
        exec ${ray} ...$args
      }
    '';
  };

  # The Ray GCS this node's kernel attaches to: the head's own tailscale IP on
  # the head (resolved at runtime as `$ip`), the configured head address on a
  # worker. `address="auto"` cannot discover the cluster because the daemon uses
  # a non-default `--temp-dir`, so the launcher hands the kernel an explicit
  # RAY_ADDRESS (which `fleet.connect()` reads).
  rayAddrNu =
    if cfg.role == "head"
    then ''$"($ip):${toString cfg.gcsPort}"''
    else ''"${cfg.headAddress}:${toString cfg.gcsPort}"'';

  # Resolve this node's tailscale IPv4, bind the dashboard/exec to it
  # (IX_MCP_HOST), point the kernel at the local Ray GCS (RAY_ADDRESS), then exec
  # the engine. systemd units get a minimal PATH, so tailscale must be a runtime
  # input here rather than relied on from the system profile.
  notebookLauncher = indexLib.writeNushellApplication pkgs {
    name = "ix-ray-notebook-launch";
    meta.description = "Resolve this node's tailscale IPv4, bind ix-mcp to it + the local Ray GCS, and exec it";
    runtimeInputs = [
      pkgs.tailscale
      cfg.notebookPackage
    ];
    text = ''
      # nu
      def main [] {
        let ip = (do --ignore-errors {
          ^tailscale ip -4 | lines | where ($it | str trim | is-not-empty) | first
        } | default "")
        if ($ip | str trim | is-empty) {
          print --stderr "ix-ray-notebook: no tailscale IPv4 yet; is tailscaled up?"
          exit 1
        }
        $env.IX_MCP_HOST = $ip
        $env.RAY_ADDRESS = ${rayAddrNu}
        exec ${lib.getExe' cfg.notebookPackage "ix-notebook"}
      }
    '';
  };

  # The ix-mcp engine (no MCP transport: `notebook` is the daemon shape). Its
  # kernel attaches to the local Ray (RAY_ADDRESS, set by the launcher), and its
  # dashboard data API exposes the `/api/exec` that `fleet.in_kernel` reaches on
  # `execPort`. Tailnet-trust is on by default (the tailnet is the boundary,
  # exactly as Ray's own data plane); set a tokenFile to also require a token.
  notebookEnv =
    {
      IX_MCP_DASHBOARD_PORT = toString cfg.execPort;
      IX_FLEET_EXEC_PORT = toString cfg.execPort;
      IX_MCP_MESH_PORT = toString cfg.meshPort;
    }
    // optionalAttrs cfg.execTrustNetwork {
      IX_MCP_EXEC_TRUST_NETWORK = "1";
    }
    // optionalAttrs (cfg.tokenFile != null) {
      IX_MCP_EXEC_TOKEN_FILE = toString cfg.tokenFile;
    };
in {
  options.services.ix-ray = {
    enable = mkEnableOption "Ray cluster node + ix-mcp engine for the `fleet` distributed API";

    role = mkOption {
      type = types.enum [
        "head"
        "worker"
      ];
      default = "worker";
      description = ''
        This node's role in the single cluster. Exactly one node must be `head`
        (it runs the GCS); every other node is a `worker` and must set
        {option}`services.ix-ray.headAddress`. The GCS is not HA (no external
        Redis), so a head restart resets cluster state: in-flight tasks and
        object refs are lost, and every raylet re-registers on its
        `Restart=on-failure` -- a reset, not an orphaned cluster.
      '';
    };

    headAddress = mkOption {
      type = types.nullOr types.str;
      default = null;
      example = "100.64.0.1";
      description = ''
        The head node's tailscale IPv4. Required on workers (they join
        `<headAddress>:<gcsPort>`); must be null on the head.
      '';
    };

    package = mkPackageOption pkgs.python3Packages "ray" {
      extraDescription = ''
        The `ray` daemon, from the same nixpkgs pin as the ray bundled in the
        ix-mcp interpreter, so the cluster and the kernels driving it run an
        identical Ray version (required cluster-wide).
      '';
    };

    notebookPackage = mkOption {
      type = types.nullOr types.package;
      default = null;
      example = lib.literalExpression "inputs.index.packages.\${pkgs.system}.mcp";
      description = ''
        The ix-mcp package providing the `ix-notebook` engine binary. Required
        when {option}`services.ix-ray.notebook.enable` is true (the default).
        Handed in by the consumer so this module needs no flake input of its own.
      '';
    };

    gcsPort = mkOption {
      type = types.port;
      default = 6379;
      description = "Head GCS port; workers connect here.";
    };

    clientServerPort = mkOption {
      type = types.port;
      default = 10001;
      description = ''
        Head Ray Client server port. Off-cluster drivers reach the cluster at
        `ray://<headAddress>:<this>` (what `fleet.connect()` uses via
        `IX_FLEET_RAY_ADDRESS`). Only the head listens here.
      '';
    };

    nodeManagerPort = mkOption {
      type = types.port;
      default = 6380;
      description = "Ray node-manager port (inter-node scheduling). Pinned so the firewall can name it.";
    };

    objectManagerPort = mkOption {
      type = types.port;
      default = 6381;
      description = "Ray object-manager port (object-store transfers). Pinned so the firewall can name it.";
    };

    workerPortLow = mkOption {
      type = types.port;
      default = 10002;
      description = "Low end of the pinned per-worker port range (inter-node worker RPC).";
    };

    workerPortHigh = mkOption {
      type = types.port;
      default = 10031;
      description = "High end of the pinned per-worker port range.";
    };

    execPort = mkOption {
      type = types.port;
      default = 8799;
      description = ''
        Port the node's ix-mcp data API (and its `/api/exec`) binds. Must match
        the `fleet` module's `IX_FLEET_EXEC_PORT` (it defaults to this) so peers
        reach each other's live kernels.
      '';
    };

    meshPort = mkOption {
      type = types.port;
      default = 8798;
      description = ''
        Port the node's ix-mcp serves its tailnet `/mesh` discovery card on
        (ix-mcp's DEFAULT_MESH_PORT). Exported to the engine as
        `IX_MCP_MESH_PORT` so the service and the firewall rule below cannot
        drift, and opened by {option}`services.ix-ray.openFirewall` when the
        notebook engine runs -- a firewall that guards the tailscale interface
        would otherwise silently blind mesh discovery.
      '';
    };

    objectStoreMemory = mkOption {
      type = types.nullOr types.ints.positive;
      default = null;
      example = 8000000000;
      description = ''
        Bytes of RAM for this node's object store (Plasma). Null lets Ray
        autodetect (~30% of RAM). Either way Ray spills to disk under
        `${spillDir}` when the store fills, so referenced objects are not lost.
        Pin it on a memory-contended host to bound Ray's share.
      '';
    };

    notebook.enable = mkOption {
      type = types.bool;
      default = true;
      description = ''
        Run the ix-mcp engine on this node so its kernel can drive Ray and
        `fleet.in_kernel` can reach its live session. Turn off on a node that
        should only contribute Ray compute.
      '';
    };

    execTrustNetwork = mkOption {
      type = types.bool;
      default = true;
      description = ''
        Trust the network the `/api/exec` endpoint binds (the tailnet) as the
        auth boundary, so `fleet.in_kernel` works without a shared token -- the
        same trust model Ray's own data plane already relies on. Set a
        {option}`services.ix-ray.tokenFile` to additionally require a bearer
        token (defense in depth). With neither set the endpoint stays disabled.
      '';
    };

    tokenFile = mkOption {
      type = types.nullOr types.path;
      default = null;
      description = ''
        File holding a shared bearer token that additionally gates `/api/exec`
        on top of the tailnet boundary. Hand every node the SAME secret. Null
        leaves the endpoint gated by the tailnet alone (when
        {option}`services.ix-ray.execTrustNetwork` is set). Provide it out of
        band (sops/agenix or a deployed file); it is read at service start.
      '';
    };

    openFirewall = mkOption {
      type = types.bool;
      default = false;
      description = ''
        Open the inter-node Ray ports (node/object manager, worker range), the
        exec port, the mesh discovery port (when the notebook engine runs),
        and on the head the GCS + client-server ports -- on the tailscale
        interface ONLY, never the global firewall (Ray has no per-call auth,
        so these ports must stay unreachable from any public interface).
        Usually unnecessary on a tailnet where peers reach each other
        directly, but required if a firewall guards the tailscale interface.
      '';
    };
  };

  config = mkIf cfg.enable {
    assertions = [
      {
        assertion = cfg.role == "head" -> cfg.headAddress == null;
        message = "services.ix-ray: the head node must not set headAddress.";
      }
      {
        assertion = cfg.role == "worker" -> cfg.headAddress != null;
        message = "services.ix-ray: a worker must set headAddress to the head's tailscale IP.";
      }
      {
        assertion = cfg.notebook.enable -> cfg.notebookPackage != null;
        message = "services.ix-ray: notebook.enable requires notebookPackage (the ix-mcp package).";
      }
    ];

    # Scoped to the tailscale interface, never the global firewall: Ray's GCS
    # and Client server carry NO authentication (any peer that can reach them
    # runs arbitrary code), and a fleet host can also have a PUBLIC interface.
    # A global `allowedTCPPorts` would have exposed `ray://<public-ip>:10001`
    # to the internet (index#1800 review). The daemons bind the tailscale IPv4,
    # and the tailnet is the module's stated trust boundary, so the tailscale
    # interface is exactly where these ports may open. `tailscale0` is
    # tailscaled's default (and the fleet's) interface name.
    networking.firewall.interfaces."tailscale0" = mkIf cfg.openFirewall {
      allowedTCPPorts =
        [
          cfg.execPort
          cfg.nodeManagerPort
          cfg.objectManagerPort
        ]
        # The notebook engine also serves the tailnet `/mesh` discovery card
        # (index#1787); a firewalled tailscale interface must not blind it.
        ++ optional cfg.notebook.enable cfg.meshPort
        ++ optionals (cfg.role == "head") [
          cfg.gcsPort
          cfg.clientServerPort
        ];
      allowedTCPPortRanges = [
        {
          from = cfg.workerPortLow;
          to = cfg.workerPortHigh;
        }
      ];
    };

    systemd.services.ix-ray = {
      description = "ix-ray Ray ${cfg.role}";
      after = [
        "network-online.target"
        "tailscaled.service"
      ];
      wants = ["network-online.target"];
      wantedBy = ["multi-user.target"];
      environment = {
        HOME = "/run/ray";
        RAY_DISABLE_USAGE_STATS = "1";
      };
      serviceConfig =
        indexLib.systemdHardening
        // {
          Type = "simple";
          ExecStart = lib.getExe rayLauncher;
          Restart = "on-failure";
          RestartSec = 5;
          DynamicUser = true;
          RuntimeDirectory = "ray";
          StateDirectory = "ray";
          WorkingDirectory = "/run/ray";
          # Ray's object store is host shared memory and an attaching kernel maps it
          # from outside this unit's namespace; a private /dev (hence /dev/shm) or
          # user namespace would block that, so both are disabled here.
          PrivateDevices = false;
          PrivateUsers = false;
        };
    };

    systemd.services.ix-ray-notebook = mkIf cfg.notebook.enable {
      description = "ix-mcp engine driving the fleet Ray cluster";
      after = [
        "network-online.target"
        "ix-ray.service"
      ];
      wants = ["network-online.target"];
      requires = ["ix-ray.service"];
      wantedBy = ["multi-user.target"];
      environment = notebookEnv;
      serviceConfig =
        indexLib.systemdHardening
        // {
          Type = "simple";
          ExecStart = lib.getExe notebookLauncher;
          Restart = "on-failure";
          RestartSec = 5;
          StateDirectory = "ix-ray-notebook";
          WorkingDirectory = "/var/lib/ix-ray-notebook";
          # Same shared-memory reasoning as the ray unit: the engine's kernel maps
          # the object store, so it cannot run under a private /dev or userns.
          PrivateDevices = false;
          PrivateUsers = false;
        };
    };
  };
}
