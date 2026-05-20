# Target platform applied to every image.
#
# All images run on EPYC Gen 5 (Turin, Zen 5). Setting hostPlatform.gcc.arch
# propagates -march=znver5 -mtune=znver5 to every package in the closure.
# No binary cache hits: everything builds from source.
{
  config,
  lib,
  pkgs,
  ...
}:
let
  healthCheckType = lib.types.submodule (
    { name, ... }:
    {
      options = {
        description = lib.mkOption {
          type = lib.types.str;
          default = name;
          description = "Human-readable check name shown by fleet health commands.";
        };

        from = lib.mkOption {
          type = lib.types.enum [
            "guest"
            "host"
          ];
          default = "guest";
          description = ''
            Where the command runs.

            `guest` execs through `ix shell <node> -- <command>` inside the VM.
            `host` execs `<command>` directly on the operator's machine and
            exports `IX_NODE` plus any fields returned by `ix ls` as
            `IX_NODE_<KEY>` env vars, so the command can probe the node from
            outside the VM (firewall, public IPv4, gateway path).
          '';
        };

        command = lib.mkOption {
          type = lib.types.nonEmptyListOf lib.types.str;
          description = ''
            Command argv. For `from = "guest"` it runs in the VM through
            `ix shell`. For `from = "host"` it runs directly with the
            `IX_NODE*` env vars described above; tools must be on the
            operator's PATH.
          '';
        };

        timeoutSec = lib.mkOption {
          type = lib.types.ints.positive;
          default = 30;
          description = "Per-attempt timeout in seconds.";
        };

        attempts = lib.mkOption {
          type = lib.types.ints.positive;
          default = 30;
          description = "Maximum number of attempts before the check fails.";
        };

        intervalSec = lib.mkOption {
          type = lib.types.ints.unsigned;
          default = 2;
          description = "Seconds to wait between failed attempts.";
        };

        requiresIpv4 = lib.mkOption {
          type = lib.types.bool;
          default = false;
          description = ''
            Whether this check needs `IX_NODE_IPV4` from `ix ls`.

            Use this for host-side public reachability probes that connect to
            the node's assigned IPv4 address. Fleet evaluation rejects nodes
            with this requirement unless `deployment.ipv4 = true`.
          '';
        };
      };
    }
  );

  portClaimType = lib.types.submodule (
    { name, ... }:
    {
      options = {
        protocol = lib.mkOption {
          type = lib.types.enum [
            "tcp"
            "udp"
          ];
          description = "Transport protocol claimed by this listener.";
        };

        port = lib.mkOption {
          type = lib.types.port;
          description = "Port claimed by this listener.";
        };

        address = lib.mkOption {
          type = lib.types.str;
          default = "*";
          description = "Bind address. Use * when the service binds every address or the bind behavior is implicit.";
        };

        namespace = lib.mkOption {
          type = lib.types.str;
          default = "default";
          description = "Network namespace for this listener. Ordinary image services use the default namespace.";
        };

        description = lib.mkOption {
          type = lib.types.str;
          default = name;
          description = "Human-readable listener owner used in collision errors.";
        };
      };
    }
  );

  portClaims = lib.mapAttrsToList (
    name: claim: claim // { inherit name; }
  ) config.ix.networking.portClaims;
  claimKey = claim: "${claim.namespace}/${claim.protocol}/${toString claim.port}";
  portClaimGroups = builtins.groupBy claimKey portClaims;
  isIpv4Address = address: lib.hasInfix "." address;
  isIpv6Address = address: lib.hasInfix ":" address;
  addressOverlaps =
    left: right:
    left == "*"
    || right == "*"
    || left == right
    || (left == "0.0.0.0" && !(isIpv6Address right))
    || (right == "0.0.0.0" && !(isIpv6Address left))
    || (left == "::" && !(isIpv4Address right))
    || (right == "::" && !(isIpv4Address left));
  groupConflicts =
    claims:
    lib.any (
      left: lib.any (right: left.name != right.name && addressOverlaps left.address right.address) claims
    ) claims;
  conflictingPortClaimGroups = lib.filterAttrs (_: groupConflicts) portClaimGroups;
  renderPortClaim = claim: "${claim.name} (${claim.address}, ${claim.description})";
  renderPortClaimConflict =
    key: claims: "${key}: ${lib.concatMapStringsSep ", " renderPortClaim claims}";
  ipv4GuestHealthChecks = lib.filterAttrs (
    _name: check: check.requiresIpv4 && check.from != "host"
  ) config.ix.healthChecks;
in
{
  options.ix = {
    healthChecks = lib.mkOption {
      type = lib.types.attrsOf healthCheckType;
      default = { };
      description = ''
        Commands that prove this image's important services are ready.

        Each check declares whether it runs from inside the VM (`from = "guest"`)
        or from the operator host (`from = "host"`); host checks are how you
        prove public reachability, firewall correctness, and external routing,
        not just that systemd thinks the unit is active. Fleet plans expose
        these so `ix-fleet health` and the post-deploy waits in `up`,
        `replace`, and `switch` can use them.
      '';
    };

    networking = {
      portClaims = lib.mkOption {
        type = lib.types.attrsOf portClaimType;
        default = { };
        description = ''
          Sockets claimed by repo-owned service modules inside this image.

          The registry catches same-namespace listener collisions at eval time.
          Use separate fleet nodes or an explicit alternate port when two services
          need the same public protocol port.
        '';
      };

      # Networking policy (per-port filtering, L7, WAF, rate limiting, gateway
      # behavior) belongs to the image, not to ix. ix exposes two primitives:
      # east-west group membership (which VMs can reach each other) and
      # north-south on/off (whether the VM has internet ingress / egress).
      # Anything finer lives in `networking.firewall.*` inside the image, in a
      # sidecar, or behind a user-built gateway VM. `eastWest.hostName` stays
      # here because it is a name, not a policy.
      eastWest.hostName = lib.mkOption {
        type = lib.types.str;
        default = config.networking.hostName;
      };
    };
  };

  config = {
    ix.networking.portClaims = {
      ix-console = {
        protocol = "tcp";
        port = 5001;
        address = "*";
        description = "ix-console shell and terminal snapshot listener";
      };

      ix-agent = {
        protocol = "udp";
        port = 8443;
        address = "*";
        description = "ix-agent WebTransport direct-connect endpoint";
      };
    };

    assertions = [
      {
        assertion = conflictingPortClaimGroups == { };
        message = ''
          ix.networking.portClaims has same-namespace port collisions:
            ${lib.concatStringsSep "\n  " (
              lib.mapAttrsToList renderPortClaimConflict conflictingPortClaimGroups
            )}

          Put services that need the same public protocol port in separate fleet nodes/VMs, or choose an explicit alternate port when same-image co-location is intentional.
        '';
      }
      {
        assertion = ipv4GuestHealthChecks == { };
        message = ''
          ix.healthChecks can only set requiresIpv4 on host checks:
            ${lib.concatStringsSep ", " (lib.attrNames ipv4GuestHealthChecks)}
        '';
      }
    ];

    nixpkgs.hostPlatform = {
      system = "x86_64-linux";
      gcc = {
        arch = "znver5";
        tune = "znver5";
      };
    };

    boot = {
      isContainer = true;

      # /tmp on tmpfs (RAM-backed). The default on-disk /tmp grows
      # without bound on long-running VMs, and ix VMs have plenty of RAM
      # to spare for ephemeral working space (see AGENTS.md "VM
      # assumptions"). Default tmpfs size is 50% of RAM, allocated lazily
      # so unused space costs nothing. Workloads that genuinely need
      # persistent or oversized temp scratch should write to /var/tmp,
      # which stays on the rootfs and is not affected by this.
      tmp = {
        useTmpfs = true;
        # No-op when useTmpfs is on, but keeps /tmp clean if an image
        # ever sets useTmpfs = false explicitly.
        cleanOnBoot = true;
      };
    };

    # Many ix VMs are SSH'd into and used as interactive dev machines, where
    # operators run unpatched prebuilt binaries (npm-installed CLIs, LSPs,
    # downloaded toolchains) that expect a standard FHS dynamic linker. Off
    # by default for the rare image that is genuinely a sealed appliance and
    # wants to drop the stub from its closure.
    programs.nix-ld.enable = lib.mkDefault true;

    # Nushell is the operator shell across this repo: the base profile ships
    # system-wide nu config and the workspace login.nu. Setting it as the
    # default user shell means service users (minecraft, ...) and any future
    # user inherit Nushell rather than bash, so an su or a manual
    # session into a service account lands in the same shell as root.
    users.defaultUserShell = pkgs.nushell;

    networking = {
      # ix provisions the guest address, route, and DNS before systemd reaches
      # normal service startup. Leaving NixOS DHCP enabled makes dhcpcd wait
      # for a lease that will never arrive, which keeps network-online.target
      # pending and blocks services such as minecraft.
      useDHCP = false;

      # In-guest firewall is the NixOS nftables backend, enforcing each
      # module's `services.*.openFirewall` and `networking.firewall.allowed*`
      # declarations. ix VMs are `boot.isContainer = true` and share the
      # host's linux-ix kernel (CONFIG_NF_TABLES); nft rules run in this
      # container's own net namespace.
      #
      # This is the primary mechanism for port-level policy. ix provides only
      # the coarse primitives (east-west group membership, north-south
      # on/off); per-port allowlists, L7, WAF, rate limiting, etc. live here
      # in the image or in a user-built gateway VM. The "primitives only"
      # rule is recorded in `ix/AGENTS.md` under "Architecture that must not
      # drift". Tracking the ix-side north-south primitive in
      # https://github.com/indexable-inc/index/issues/41.
      nftables.enable = true;
      firewall = {
        enable = lib.mkDefault true;
        allowedTCPPorts = [
          5001 # ix-console shell and terminal snapshot listener.
        ];
        allowedUDPPorts = [
          8443 # ix-agent WebTransport direct-connect endpoint.
        ];
      };
    };

    services = {
      # Bound the journal so a long-running VM that catches one tcpdump-style
      # spam burst does not fill its disk with rotated journal files.
      # Override per image when an operator actually needs the historical
      # depth.
      journald.extraConfig = lib.mkDefault ''
        SystemMaxUse=1G
      '';

      # Varlink multiplexer for JSON user/group records. On-demand activated,
      # so the cost when nothing queries it is negligible. Having it on means
      # operator-side `userdbctl user/group` works out of the box, services
      # that adopt `DynamicUser=true` get proper cgroup accounting through a
      # synthesized record, and the eventual NFTSet=/cgroup-id integration
      # over the nftables backend already enabled here can hook into real
      # user/group identities without another platform-wide flip.
      userdbd = {
        enable = true;
        # macOS Nix installs (and other multi-user setups) ship `nixbld*`
        # build users above 1000, which trips the systemd-userdb "regular
        # users have system-range UIDs" warning at eval time. The build
        # users do not exist inside ix VMs at all — `boot.isContainer =
        # true` means the guest has its own user namespace — so the
        # warning is noise from the host nixpkgs evaluator rather than a
        # runtime concern. Silencing here keeps `nix run .#health-checks`
        # and other repo evals scannable.
        silenceHighSystemUsers = true;
      };
    };

    # Capture native crashes (JVM segfault, Rust panics in extern, anything
    # that takes SIGSEGV/SIGABRT) into /var/lib/systemd/coredump where
    # `coredumpctl list/info/gdb` can find them. Without this, a crashed
    # service shows `signal=SEGV` in the journal and the dump goes to
    # /dev/null. MaxUse caps a crash-looping service rather than saving
    # disk — disk autoscales to ~1 PiB, the cap is just defence in depth.
    systemd.coredump = {
      enable = true;
      settings.Coredump = {
        Storage = "external";
        Compress = "yes";
        MaxUse = "5G";
      };
    };

    # Modern Nix configuration for any in-VM nix invocation.
    #
    # experimental-features: nix CLI, flakes, the `|>` pipe operator,
    # fetchClosure, content-addressed derivations, dynamic derivations,
    # and git tree hashing. List-typed, so per-image additions concatenate
    # rather than override.
    #
    # allow-import-from-derivation: IFD is allowed by upstream default;
    # setting it explicitly pins repo policy ("IFD is allowed when it
    # removes a fake Nix layer", AGENTS.md) against a future upstream flip.
    #
    # gc + optimise: long-lived dev VMs accumulate roots from `nix run` /
    # `nix shell` and never get a /nix/store sweep otherwise. 30 days
    # keeps recent results for repeat invocations and bounds disk growth.
    # Optimise hardlinks duplicate store paths so the savings compound.
    nix = {
      settings = {
        experimental-features = [
          "nix-command"
          "flakes"
          "pipe-operators"
          "fetch-closure"
          "fetch-tree"
          "ca-derivations"
          "dynamic-derivations"
          "git-hashing"
        ];
        allow-import-from-derivation = lib.mkDefault true;
        warn-dirty = false;
      };
      gc = {
        automatic = true;
        options = "--delete-older-than 30d";
      };
      optimise.automatic = true;
      # Keep ad-hoc nix work an operator runs over SSH from starving the
      # service workload (Minecraft tick rate, Postgres queries). The
      # daemon still makes progress; the kernel just deprioritizes it
      # whenever the real service wants CPU or disk.
      daemonCPUSchedPolicy = "idle";
      daemonIOSchedClass = "idle";
    };

    system.stateVersion = "25.05";
  };
}
