/**
  One Ray cluster node as a NixOS module.

  Head and worker nodes share everything except the `ray start` mode: the same
  package, the same pinned ports, the same `nix-ld` environment, and the same
  hardened long-running service. Callers pass `role` (the systemd unit suffix
  and `ray start` subcommand shape) and `extraStartArgs` for the mode-specific
  flags (`--head` and the GCS port on the head, `--address` on a worker).

  Ports are pinned rather than left to Ray's default random high range so the
  guest firewall can name them. `node-manager`, `object-manager`, and the
  worker port range are opened here because every node listens on them; the
  head opens its GCS and client ports in `head.nix`.
*/
{
  ix,
  lib,
  pkgs,
  role,
  extraStartArgs,
  rayAddress,
}:
{ config, ... }:
let
  package = import ./package.nix { inherit ix lib pkgs; };
  rayCli = import ./cli.nix { inherit ix lib pkgs rayAddress; };
  # buildUvApplication wraps only the `ray-demo` main program; the Ray CLI
  # itself lives unwrapped in the venv, so reference it directly and set the
  # loader environment on the unit below.
  ray = "${package}/venv/bin/ray";

  ports = {
    gcs = 6379;
    nodeManager = 6380;
    objectManager = 6381;
    client = 10001;
    workerLow = 10002;
    workerHigh = 10031;
  };

  # `_raylet.so` (dlopened by the Python process) resolves through the normal
  # loader, so libstdc++ and zlib go on LD_LIBRARY_PATH. The standalone
  # `raylet`/`gcs_server` binaries Ray execs are FHS ELF objects: their
  # PT_INTERP is the stock `/lib64/ld-linux`, which the image's `nix-ld` stub
  # serves, reading NIX_LD/NIX_LD_LIBRARY_PATH. systemd units do not inherit
  # the session `environment.variables` nix-ld sets, so set them on the unit.
  loaderLibraryPath = lib.makeLibraryPath [
    pkgs.stdenv.cc.cc.lib
    pkgs.zlib
  ];
  nixLdLib = "/run/current-system/sw/share/nix-ld/lib";

  # A short temp-dir keeps Ray's AF_UNIX socket paths under the 108-byte
  # `sun_path` limit; a DynamicUser StateDirectory under /var/lib/private is
  # long enough to overflow it once Ray appends its session and socket names.
  tempDir = "/run/ray";

  commonStartArgs = [
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

  startArgs = [ ray "start" ] ++ extraStartArgs ++ commonStartArgs ++ [ "--block" ];
in
{
  environment.systemPackages = [ rayCli ];

  systemd.services."ray-${role}" = {
    description = "Ray cluster ${role}";
    wantedBy = [ "multi-user.target" ];
    after = [ "network-online.target" ];
    wants = [ "network-online.target" ];
    environment = {
      LD_LIBRARY_PATH = loaderLibraryPath;
      NIX_LD = "${nixLdLib}/ld.so";
      NIX_LD_LIBRARY_PATH = "${loaderLibraryPath}:${nixLdLib}";
      HOME = tempDir;
      RAY_DISABLE_USAGE_STATS = "1";
    };
    serviceConfig =
      ix.systemdHardening
      // {
        ExecStart = lib.escapeShellArgs startArgs;
        ExecStop = lib.escapeShellArgs [
          ray
          "stop"
          "--grace-period"
          "10"
        ];
        Restart = "on-failure";
        RestartSec = 5;
        DynamicUser = true;
        RuntimeDirectory = "ray";
        WorkingDirectory = tempDir;
      };
  };

  networking.firewall = {
    allowedTCPPorts = [
      ports.nodeManager
      ports.objectManager
    ];
    allowedTCPPortRanges = [
      {
        from = ports.workerLow;
        to = ports.workerHigh;
      }
    ];
  };

  ix.networking.portClaims = {
    ray-node-manager = {
      protocol = "tcp";
      port = ports.nodeManager;
      description = "Ray node manager (inter-node scheduling)";
    };
    ray-object-manager = {
      protocol = "tcp";
      port = ports.objectManager;
      description = "Ray object manager (object store transfers)";
    };
  };

  ix.healthChecks."ray-${role}-active" = {
    description = "ray-${role} service is active";
    command = [
      (lib.getExe' config.systemd.package "systemctl")
      "is-active"
      "--quiet"
      "ray-${role}.service"
    ];
  };
}
