# Darwin fabric worker (index#3192): joins a mac (behind an explicit enable
# flag) to the one fleet Ray cluster as a worker. The head and the full
# node-level story live in modules/services/ray/default.nix (NixOS); this is
# the deliberately smaller nix-darwin counterpart -- worker role only (macs
# never hold the GCS), no notebook engine (workstation kernels run under the
# user's own profile), same pinned inter-node ports via `indexLib.fabric.ports`
# and the same wrapped daemon env via `indexLib.fabric.rayEnv`, whose
# RAY_ENABLE_WINDOWS_OR_OSX_CLUSTER gate is what lets a darwin node join a
# multi-node cluster at all.
# `indexLib` is the index flake lib, injected at the flake's darwinModules
# wiring for the same reason modules/services/ray/default.nix documents.
{indexLib}: {
  # Same dedup story as modules/darwin/provenance.nix: applying the injection
  # makes each import site a distinct anonymous attrset, so an explicit `key`
  # restores module-system dedup and `_file` restores error attribution.
  key = "index/modules/darwin/ray.nix";
  _file = "index/modules/darwin/ray.nix";

  imports = [
    ({
      config,
      lib,
      pkgs,
      ...
    }: let
      cfg = config.services.ix-ray;

      # Worker join flags. Unlike the NixOS unit there is no explicit spill
      # directory: macOS /tmp is disk-backed (not a tmpfs), so Ray's default
      # spill location under the temp dir already lands on real disk. The
      # short /tmp path also keeps the plasma AF_UNIX socket under the
      # 108-byte sun_path limit.
      startArgs =
        [
          "start"
          "--address"
          "${cfg.headAddress}:${toString cfg.gcsPort}"
          "--node-manager-port"
          (toString cfg.nodeManagerPort)
          "--object-manager-port"
          (toString cfg.objectManagerPort)
          "--min-worker-port"
          (toString cfg.workerPortLow)
          "--max-worker-port"
          (toString cfg.workerPortHigh)
          "--temp-dir"
          "/tmp/ix-ray"
          # Same data as the NixOS module: host label, os label, env
          # handshake resource (see `indexLib.fabric.nodeResources`).
          "--resources"
          (builtins.toJSON cfg.resources)
        ]
        ++ lib.optionals (cfg.objectStoreMemory != null) [
          "--object-store-memory"
          (toString cfg.objectStoreMemory)
        ];

      startArgsNu = "[ ${lib.concatMapStringsSep " " builtins.toJSON startArgs} ]";

      # Resolve the node's tailscale IPv4 at runtime and exec the daemon bound
      # to it, mirroring the NixOS launcher. The tailscale CLI is host state on
      # darwin (the standalone install or the app bundle), not a Nix input:
      # only the CLI matching the running tailscaled can query it.
      launcher = indexLib.writeNushellApplication pkgs {
        name = "ix-ray-launch";
        meta.description = "Resolve this mac's tailscale IPv4 and exec the ray worker daemon bound to it";
        runtimeInputs = [
          cfg.package
          # Same constraint as the NixOS launcher: ray execs worker wrappers
          # via `os.execvp("bash", ...)`, so bash must be on the daemon PATH.
          pkgs.bash
        ];
        text = ''
          # nu
          def main [] {
            let candidates = [
              "/usr/local/bin/tailscale"
              "/Applications/Tailscale.app/Contents/MacOS/Tailscale"
            ]
            let found = ($candidates | where ($it | path exists))
            if ($found | is-empty) {
              print --stderr $"ix-ray: no tailscale CLI at any of ($candidates)"
              exit 1
            }
            let ts = ($found | first)
            let ip = (do --ignore-errors {
              ^$ts ip -4 | lines | where ($it | str trim | is-not-empty) | first
            } | default "")
            if ($ip | str trim | is-empty) {
              print --stderr "ix-ray: no tailscale IPv4 yet; is tailscale up?"
              exit 1
            }
            let args = [ ...${startArgsNu} "--node-ip-address" $ip "--block" ]
            exec ${lib.getExe' cfg.package "ray"} ...$args
          }
        '';
      };
    in {
      options.services.ix-ray = {
        enable = lib.mkEnableOption "joining this mac to the fleet Ray cluster as a fabric worker";

        headAddress = lib.mkOption {
          type = lib.types.str;
          example = "100.64.0.1";
          description = "The head node's tailscale IPv4 (see the NixOS module's `role = \"head\"`).";
        };

        package = lib.mkOption {
          type = lib.types.package;
          default = indexLib.fabric.rayEnv pkgs;
          defaultText = lib.literalExpression "indexLib.fabric.rayEnv pkgs";
          description = ''
            The `ray` daemon: the pinned fabric env (same nixpkgs ray as the
            ix-mcp interpreter, wrapped with the cluster env vars).
          '';
        };

        resources = lib.mkOption {
          type = lib.types.attrsOf lib.types.number;
          default = {};
          example = {ssd = 1;};
          description = ''
            Ray custom resources this node advertises. The fabric baseline
            (`host_<hostName>`, `darwin`, the `fabric_env:<tag>` handshake
            resource) is merged in below; set extra keys here for
            host-specific capabilities.
          '';
        };

        gcsPort = lib.mkOption {
          type = lib.types.port;
          default = indexLib.fabric.ports.gcs;
          description = "Head GCS port this worker joins.";
        };

        nodeManagerPort = lib.mkOption {
          type = lib.types.port;
          default = indexLib.fabric.ports.nodeManager;
          description = "Ray node-manager port (inter-node scheduling), pinned fleet-wide.";
        };

        objectManagerPort = lib.mkOption {
          type = lib.types.port;
          default = indexLib.fabric.ports.objectManager;
          description = "Ray object-manager port (object-store transfers), pinned fleet-wide.";
        };

        workerPortLow = lib.mkOption {
          type = lib.types.port;
          default = indexLib.fabric.ports.workerLow;
          description = "Low end of the pinned per-worker port range (inter-node worker RPC).";
        };

        workerPortHigh = lib.mkOption {
          type = lib.types.port;
          default = indexLib.fabric.ports.workerHigh;
          description = "High end of the pinned per-worker port range.";
        };

        objectStoreMemory = lib.mkOption {
          type = lib.types.nullOr lib.types.ints.positive;
          default = null;
          example = 8_000_000_000;
          description = ''
            Bytes of RAM for this node's object store (Plasma). Null lets Ray
            autodetect. The fabric env lifts Ray's 2GiB macOS cap
            (RAY_ENABLE_MAC_LARGE_OBJECT_STORE).
          '';
        };
      };

      config = lib.mkIf cfg.enable {
        assertions = [
          {
            assertion = config.networking.hostName != null;
            message = "services.ix-ray (darwin): networking.hostName must be set; it names this node's `host_<name>` fabric label.";
          }
        ];

        services.ix-ray.resources = indexLib.fabric.nodeResources {
          python = pkgs.python3;
          hostName = config.networking.hostName;
          os = "darwin";
        };

        launchd.daemons.ix-ray = {
          serviceConfig = {
            ProgramArguments = [(lib.getExe launcher)];
            RunAtLoad = true;
            KeepAlive = true;
            # Root's real home: the nushell launcher needs a writable
            # $HOME/.config/nushell and ray writes only under its temp dir.
            EnvironmentVariables = {
              HOME = "/var/root";
              RAY_DISABLE_USAGE_STATS = "1";
            };
            StandardOutPath = "/var/log/ix-ray.log";
            StandardErrorPath = "/var/log/ix-ray.log";
          };
        };
      };
    })
  ];
}
