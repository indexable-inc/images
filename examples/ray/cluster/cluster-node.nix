/**
  One Ray cluster node as a NixOS module.

  Head and worker nodes share everything except the `ray start` mode: the same
  package, the same pinned ports, the same `nix-ld` environment, and the same
  long-running service. Callers pass `role` (the systemd unit suffix) and
  `extraStartArgs` for the mode-specific flags (`--head` and the GCS port on the
  head, `--address` on a worker). `rayAddress` is the head's `host:port`, used by
  the wrapped CLI on this node.

  Ports are pinned rather than left to Ray's default random high range so the
  guest firewall can name the inter-node ones. `node-manager`, `object-manager`,
  and the worker port range are opened here because every node listens on them;
  the head opens its GCS and client ports in `head.nix`. Ray also runs node-local
  agents (dashboard agent, metrics, runtime-env) on other ports; those are not
  reached across nodes here, so they are left unexposed.
*/
{
  ix,
  lib,
  pkgs,
  role,
  extraStartArgs,
  rayAddress,
}:
_:
let
  package = import ./package.nix { inherit ix lib pkgs; };
  rayCli = import ./cli.nix {
    inherit
      ix
      lib
      pkgs
      rayAddress
      ;
  };
  loader = import ./loader.nix { inherit lib pkgs; };

  ports = {
    nodeManager = 6380;
    objectManager = 6381;
    workerLow = 10002;
    workerHigh = 10031;
  };

  # A short temp-dir keeps Ray's AF_UNIX socket paths under the 108-byte
  # `sun_path` limit; a DynamicUser StateDirectory under /var/lib/private is long
  # enough to overflow it once Ray appends its session and socket names.
  tempDir = "/run/ray";

  rayStartArgs = [
    "start"
  ]
  ++ extraStartArgs
  ++ [
    "--node-manager-port"
    (toString ports.nodeManager)
    "--object-manager-port"
    (toString ports.objectManager)
    "--min-worker-port"
    (toString ports.workerLow)
    "--max-worker-port"
    (toString ports.workerHigh)
    "--temp-dir"
    tempDir
  ];

  # Render the arg list as a Nushell list literal so the start script can splat
  # it; each element is JSON-quoted, which is valid Nu string syntax.
  rayStartArgsNu = "[ ${lib.concatMapStringsSep " " builtins.toJSON rayStartArgs} ]";

  # Ray's default node-IP autodetect opens a UDP socket toward a public resolver
  # and falls back to 127.0.0.1 when that fails. These nodes are east-west only
  # with no internet egress, so derive the address from the routing table
  # instead and bind Ray to the interface workers actually reach.
  startScript = ix.writeNushellApplication pkgs {
    name = "ray-${role}-start";
    runtimeInputs = [
      pkgs.iproute2
      pkgs.coreutils
    ];
    text = ''
      # nu
      let node_ip = (
        ip -4 -o addr show scope global
        | lines
        | parse --regex 'inet (?<ip>\d+\.\d+\.\d+\.\d+)'
        | get ip?
        | get 0?
        | default ""
      )
      if ($node_ip | is-empty) {
        print --stderr "ray-${role}: no global IPv4 address found"
        exit 1
      }
      let ray_args = [ ...${rayStartArgsNu} "--node-ip-address" $node_ip "--block" ]
      exec ${package}/venv/bin/ray ...$ray_args
    '';
  };
in
{
  environment.systemPackages = [ rayCli ];

  systemd.services."ray-${role}" = {
    description = "Ray cluster ${role}";
    wantedBy = [ "multi-user.target" ];
    after = [ "network-online.target" ];
    wants = [ "network-online.target" ];
    environment = {
      LD_LIBRARY_PATH = loader.libraryPath;
      NIX_LD = loader.nixLd;
      NIX_LD_LIBRARY_PATH = loader.nixLdLibraryPath;
      HOME = tempDir;
      RAY_DISABLE_USAGE_STATS = "1";
    };
    serviceConfig = ix.systemdHardening // {
      ExecStart = lib.getExe startScript;
      # SIGTERM to the foreground `--block` process is Ray's shutdown path;
      # `ray stop` cannot see its own processes under ProtectProc and races
      # the RuntimeDirectory teardown, so there is no ExecStop.
      Restart = "on-failure";
      RestartSec = 5;
      DynamicUser = true;
      RuntimeDirectory = "ray";
      WorkingDirectory = tempDir;
      # Ray's object store is host shared memory, and the health-check driver
      # attaches from outside this unit's namespace. A private /dev (hence a
      # private /dev/shm) or a private user namespace would stop that driver
      # from mapping the store, so both are disabled for this service.
      PrivateDevices = false;
      PrivateUsers = false;
    };
  };

  ix.networking.expose = {
    ray-node-manager = {
      port = ports.nodeManager;
      description = "Ray node manager (inter-node scheduling)";
    };
    ray-object-manager = {
      port = ports.objectManager;
      description = "Ray object manager (object store transfers)";
    };
  };

  # The worker port range is opened at the firewall directly: `expose` covers a
  # single named listener, not a range.
  networking.firewall.allowedTCPPortRanges = [
    {
      from = ports.workerLow;
      to = ports.workerHigh;
    }
  ];

  ix.healthChecks."ray-${role}-active" = {
    description = "ray-${role} service is active";
    unit = "ray-${role}";
  };
}
