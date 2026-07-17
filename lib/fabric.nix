/**
The pinned fabric execution environment (index#3192, design index#3190): the
one owner of how a fabric/Ray cluster node identifies and configures itself.
Consumed by the NixOS cluster module ([`modules/services/ray`](modules/services/ray/default.nix)),
the darwin worker module ([`modules/darwin/ray.nix`](modules/darwin/ray.nix)),
and the ix-mcp wrappers ([`packages/mcp`](packages/mcp/default.nix)), so the
daemons, the kernels driving them, and the submit-time handshake can never
disagree about the env.

Identity is deliberately platform-independent: cloudpickled bytecode travels
across OS/arch when (and only when) the python and ray versions match, so the
tag is `py<maj.min>-ray<version>` -- store paths would differ per platform and
falsely fail the darwin -> linux handshake. ray vendors its own cloudpickle
(`ray.cloudpickle`), so pinning ray pins cloudpickle with it; one nixpkgs pin
gives darwin and linux identical python/ray/cloudpickle.
*/
{lib}: let
  # Ray refuses multi-node participation on darwin without this gate, and it
  # is read at `ray_constants` import time by EVERY ray process -- the daemon
  # joining as a worker AND a driver attaching to a local raylet (without it a
  # darwin driver binds loopback instead of its routable address). The large
  # object store is capped to 2GiB on macOS unless the second var lifts it.
  # These are set ONLY through the Nix wrappers below, never in user shells.
  clusterEnv = {
    RAY_ENABLE_WINDOWS_OR_OSX_CLUSTER = "1";
    RAY_ENABLE_MAC_LARGE_OBJECT_STORE = "1";
  };

  envTag = python: "py${lib.versions.majorMinor python.version}-ray${python.pkgs.ray.version}";

  envResource = python: "fabric_env:${envTag python}";
in {
  inherit clusterEnv envTag envResource;

  /**
  The pinned inter-node port claims of the one fleet Ray cluster, shared by
  the NixOS module and the darwin worker module so peers (and their
  firewalls) can never disagree about the fixed raylet port range. The
  notebook/exec ports are NixOS-only and stay with that module.
  */
  ports = {
    gcs = 6379;
    clientServer = 10001;
    nodeManager = 6380;
    objectManager = 6381;
    workerLow = 10002;
    workerHigh = 10031;
  };

  /**
  The Ray custom resources a fabric node advertises, as data: its addressable
  host label (`host_<name>`, what `fabric.run(node=...)` targets), its OS
  label, the env-handshake resource, and `gpu` when the host has one. Rendered
  to `ray start --resources` JSON by the modules.
  */
  nodeResources = {
    python,
    hostName,
    os,
    gpu ? false,
  }:
    assert lib.assertMsg (lib.elem os ["linux" "darwin"]) "ix.fabric.nodeResources: os must be \"linux\" or \"darwin\", got ${toString os}";
      {
        "host_${hostName}" = 1;
        "${os}" = 1;
        "${envResource python}" = 1;
      }
      // lib.optionalAttrs gpu {gpu = 1;};

  /**
  Env vars for a kernel/driver process (the ix-mcp wrappers): the cluster
  gate plus `IX_FABRIC_ENV`, this env's handshake resource name, which
  `fabric.remote` compares against the target node's advertised resource at
  submit. `python` must be the interpreter the process actually runs.
  */
  kernelEnv = python:
    clusterEnv
    // {
      IX_FABRIC_ENV = envResource python;
    };

  /**
  The `ray` daemon CLI of the pinned fabric env: the same nixpkgs ray the
  ix-mcp interpreter imports (Ray requires matching versions cluster-wide),
  wrapped with the cluster env vars so one package serves darwin and linux
  workers alike, plus git on PATH for the runner actor's per-run workspace
  clones (fleet units run with a minimal PATH).
  */
  rayEnv = pkgs: let
    # The head's Ray Client server (`--ray-client-server-port`, what `ray://`
    # drivers attach through) needs ray's `client` extra (grpcio); plain
    # `ps.ray` refuses to even start with the port configured. One env for
    # every role, so head and workers cannot drift.
    env = pkgs.python3.withPackages (ps: [ps.ray] ++ ps.ray.optional-dependencies.client);
  in
    pkgs.runCommand "fabric-ray-env" {
      nativeBuildInputs = [pkgs.makeWrapper];
      strictDeps = true;
      meta = {
        description = "Pinned fabric ray daemon: nixpkgs ray wrapped with the cluster env vars";
        mainProgram = "ray";
      };
    } ''
      mkdir -p $out/bin
      makeWrapper ${lib.getExe' env "ray"} $out/bin/ray \
        ${lib.concatStringsSep " " (lib.mapAttrsToList (name: value: "--set ${name} ${lib.escapeShellArg value}") clusterEnv)} \
        --prefix PATH : ${lib.makeBinPath [pkgs.git]}
    '';
}
