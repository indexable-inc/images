# Baseline platform applied to every image.
{
  config,
  lib,
  pkgs,
  ...
}: let
  # `ix.healthChecks.<name>.unit` sugar: probe a systemd unit with
  # `systemctl is-active`. A bare name gets the `.service` suffix; pass an
  # explicit `foo.socket`/`foo.timer` to probe another unit type.
  unitName = unit:
    if lib.hasInfix "." unit
    then unit
    else "${unit}.service";
  mkUnitCommand = unit: [
    (lib.getExe' config.systemd.package "systemctl")
    "is-active"
    "--quiet"
    (unitName unit)
  ];

  # `ix.healthChecks.<name>.http` sugar: an in-guest HTTP readiness probe.
  # `--fail` maps curl's exit code onto the check result the same way a
  # Kubernetes httpGet probe treats a >= 400 status as unhealthy.
  mkHttpCommand = http: [
    (lib.getExe pkgs.curl)
    "--fail"
    "--silent"
    "--show-error"
    "http://${http.host}:${toString http.port}${http.path}"
  ];

  # `ix.healthChecks.<name>.tcp` sugar: an in-guest TCP connect probe,
  # the analog of a Kubernetes tcpSocket probe.
  mkTcpCommand = tcp: [
    (lib.getExe' pkgs.netcat-openbsd "nc")
    "-z"
    tcp.host
    (toString tcp.port)
  ];

  httpProbeType = lib.types.submodule {
    options = {
      port = lib.mkOption {
        type = lib.types.port;
        description = "Port the HTTP listener answers on.";
      };

      path = lib.mkOption {
        type = lib.types.str;
        default = "/";
        example = "/healthz";
        description = "Request path; give the service a cheap dedicated readiness route where you can.";
      };

      host = lib.mkOption {
        type = lib.types.str;
        default = "127.0.0.1";
        description = "Host to connect to from inside the guest.";
      };
    };
  };

  tcpProbeType = lib.types.submodule {
    options = {
      port = lib.mkOption {
        type = lib.types.port;
        description = "Port to open a TCP connection to.";
      };

      host = lib.mkOption {
        type = lib.types.str;
        default = "127.0.0.1";
        description = "Host to connect to from inside the guest.";
      };
    };
  };

  healthCheckType = lib.types.submodule (
    {
      name,
      config,
      ...
    }: {
      options = {
        description = lib.mkOption {
          type = lib.types.str;
          default = name;
          description = "Human-readable check name shown by fleet health commands.";
        };

        unit = lib.mkOption {
          type = lib.types.nullOr lib.types.str;
          default = null;
          example = "nginx";
          description = ''
            A systemd unit to probe with `systemctl is-active --quiet`.

            Sugar for the overwhelmingly common "is this unit running?" check:
            setting `unit` derives `command` for you, so
            `ix.healthChecks.nginx.unit = "nginx";` replaces the full
            `systemctl is-active` argv. A bare name gets the `.service` suffix;
            pass `foo.socket` or `foo.timer` to probe another unit type.

            Mutually exclusive with `command`: set one or the other, not both.
          '';
        };

        http = lib.mkOption {
          type = lib.types.nullOr httpProbeType;
          default = null;
          example = lib.literalExpression ''{ port = 8080; path = "/healthz"; }'';
          description = ''
            An in-guest HTTP readiness probe, the analog of a Kubernetes
            `httpGet` probe: setting `http` derives a curl `command` that
            succeeds on 2xx/3xx and fails on any status >= 400 (curl
            `--fail`). The probe binary is pinned into the image closure
            automatically, so no `environment.systemPackages` bookkeeping
            is needed.

            Sugar for `command`; set at most one of `unit`, `http`, `tcp`,
            or an explicit `command`.
          '';
        };

        tcp = lib.mkOption {
          type = lib.types.nullOr tcpProbeType;
          default = null;
          example = lib.literalExpression ''{ port = 5432; }'';
          description = ''
            An in-guest TCP connect probe, the analog of a Kubernetes
            `tcpSocket` probe: setting `tcp` derives an `nc -z` `command`
            that succeeds once the port accepts a connection. The probe
            binary is pinned into the image closure automatically.

            Sugar for `command`; set at most one of `unit`, `http`, `tcp`,
            or an explicit `command`.
          '';
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

            When `unit` is set this defaults to a `systemctl is-active` probe of
            that unit, so most checks only set `unit`. Set `command` for a real
            readiness probe (an HTTP request, a query) rather than a bare unit
            liveness check; set one of `unit` or `command`.
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

      # Probe sugar lives here, not in `command`'s default: a public option's
      # default must be a self-contained literal (repo astlog rule), so the
      # sugar -> command branch is seeded in config as an mkDefault a real
      # `command` (priority 100) still overrides. One definition site (first
      # set sugar wins) rather than one mkIf per sugar, so setting two sugars
      # reaches the readable platform-level assertion instead of a module
      # "conflicting definition values" error.
      config = lib.mkIf (config.unit != null || config.http != null || config.tcp != null) {
        command = lib.mkDefault (
          if config.unit != null
          then mkUnitCommand config.unit
          else if config.http != null
          then mkHttpCommand config.http
          else mkTcpCommand config.tcp
        );
      };
    }
  );

  portClaimType = lib.types.submodule (
    {name, ...}: {
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

  portClaims =
    lib.mapAttrsToList (
      name: claim: claim // {inherit name;}
    )
    config.ix.networking.portClaims;
  claimKey = claim: "${claim.namespace}/${claim.protocol}/${toString claim.port}";
  portClaimGroups = builtins.groupBy claimKey portClaims;
  isIpv4Address = address: lib.hasInfix "." address;
  isIpv6Address = address: lib.hasInfix ":" address;
  addressOverlaps = left: right:
    left
    == "*"
    || right == "*"
    || left == right
    || (left == "0.0.0.0" && !(isIpv6Address right))
    || (right == "0.0.0.0" && !(isIpv6Address left))
    || (left == "::" && !(isIpv4Address right))
    || (right == "::" && !(isIpv4Address left));
  groupConflicts = claims:
    lib.any (
      left: lib.any (right: left.name != right.name && addressOverlaps left.address right.address) claims
    )
    claims;
  conflictingPortClaimGroups = lib.filterAttrs (_: groupConflicts) portClaimGroups;
  renderPortClaim = claim: "${claim.name} (${claim.address}, ${claim.description})";
  renderPortClaimConflict = key: claims: "${key}: ${lib.concatMapStringsSep ", " renderPortClaim claims}";
  ipv4GuestHealthChecks =
    lib.filterAttrs (
      _name: check: check.requiresIpv4 && check.from != "host"
    )
    config.ix.healthChecks;

  # The declarative probe sugars (`unit`, `http`, `tcp`) each derive `command`,
  # so they are mutually exclusive with each other and with an explicit
  # `command`: a check that sets two sources of truth would silently ignore
  # one. `probeSugars` mirrors the submodule's derivation order.
  probeSugars = check: lib.filter (sugar: check.${sugar} != null) ["unit" "http" "tcp"];
  sugarCommand = check:
    if check.unit != null
    then mkUnitCommand check.unit
    else if check.http != null
    then mkHttpCommand check.http
    else mkTcpCommand check.tcp;

  multiSugarHealthChecks =
    lib.filterAttrs (
      _name: check: builtins.length (probeSugars check) > 1
    )
    config.ix.healthChecks;

  # Health checks that set a probe sugar must not also override `command`: the
  # whole point of the sugar is that it derives the command, so a custom
  # command means the sugar is silently ignored. Flag it instead of letting
  # them disagree.
  overSpecifiedHealthChecks =
    lib.filterAttrs (
      _name: check: probeSugars check != [] && check.command != sugarCommand check
    )
    config.ix.healthChecks;

  # `http`/`tcp` probes exec pinned in-guest store paths (curl, nc), which do
  # not exist on the operator's machine; a host-side reachability probe needs
  # an explicit `command` with tools from the operator's PATH.
  hostProbeSugarHealthChecks =
    lib.filterAttrs (
      _name: check: check.from == "host" && (check.http != null || check.tcp != null)
    )
    config.ix.healthChecks;

  # Probe binaries ride the system closure: the plan strips string context
  # from check argv (fleet.nix `planHealthChecks`), so nothing else retains
  # curl/nc for a check that is the only reference to them.
  healthCheckValues = lib.attrValues config.ix.healthChecks;
  healthProbePackages =
    lib.optional (lib.any (check: check.http != null) healthCheckValues) pkgs.curl
    ++ lib.optional (lib.any (check: check.tcp != null) healthCheckValues) pkgs.netcat-openbsd;

  # `ix.networking.expose.<name>` is the one declaration for "this image listens
  # here": it registers the port in the claim registry (so collisions are caught
  # at eval time) and, by default, opens the in-guest firewall for it. It also
  # makes the listener discoverable across the fleet via `ix.endpointOf`.
  exposeType = lib.types.submodule (
    {name, ...}: {
      options = {
        port = lib.mkOption {
          type = lib.types.port;
          description = "Port this image listens on.";
        };

        protocol = lib.mkOption {
          type = lib.types.enum [
            "tcp"
            "udp"
          ];
          default = "tcp";
          description = "Transport protocol of this listener.";
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
          description = "Human-readable listener owner, used in collision errors and health output.";
        };

        firewall = lib.mkOption {
          type = lib.types.bool;
          default = true;
          description = ''
            Open the in-guest firewall for this port. Leave it on for the normal
            case (this image owns the listener). Set it false when another
            mechanism already opens the port (a service's own
            `openFirewall = true`) and you only want the registry entry and
            cross-node discovery.
          '';
        };
      };
    }
  );

  exposeList = lib.attrValues config.ix.networking.expose;
  exposePortClaims =
    lib.mapAttrs (_name: e: {
      inherit
        (e)
        protocol
        port
        address
        namespace
        description
        ;
    })
    config.ix.networking.expose;
  exposeFirewallPorts = proto: map (e: e.port) (lib.filter (e: e.firewall && e.protocol == proto) exposeList);
in {
  options.ix = {
    healthChecks = lib.mkOption {
      type = lib.types.attrsOf healthCheckType;
      default = {};
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
        default = {};
        description = ''
          Sockets claimed by repo-owned service modules inside this image.

          The registry catches same-namespace listener collisions at eval time.
          Use separate fleet nodes or an explicit alternate port when two services
          need the same public protocol port.
        '';
      };

      expose = lib.mkOption {
        type = lib.types.attrsOf exposeType;
        default = {};
        example = lib.literalExpression ''
          {
            http = {
              port = 8080;
              description = "public HTTP API";
            };
          }
        '';
        description = ''
          Listeners this image exposes, declared once. Each entry registers a
          port claim (so same-namespace collisions fail at eval time), opens the
          in-guest firewall for the port (unless `firewall = false`), and becomes
          discoverable from sibling nodes via `ix.endpointOf nodes.<node> "<name>"`.

          This is the one source of truth for a port: prefer it over hand-pairing
          `networking.firewall.allowed*Ports` with `ix.networking.portClaims`,
          which is the lower-level primitive `expose` desugars to.
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

      groups = lib.mkOption {
        type = lib.types.listOf lib.types.str;
        default = [];
        example = lib.literalExpression ''[ "shared-db" ]'';
        description = ''
          East-west group slugs this image's VM joins at creation.

          Declared in the image so the network identity travels with the
          definition: the fleet plan unions these with the fleet-level
          `nodes.<name>.groups`, and the create path get-or-creates each
          slug under the deploying user before first boot. VMs sharing a
          slug reach each other privately as `<eastWest.hostName>.ix.internal`;
          a VM outside the group has no route in.

          Slugs are scoped per owner and limited to `[a-z0-9_-]`, max 63
          chars (the DNS label limit); the fleet eval rejects anything else
          before any RPC runs.
        '';
      };
    };
  };

  config = {
    ix.networking.portClaims =
      exposePortClaims
      // {
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
        assertion = conflictingPortClaimGroups == {};
        message = ''
          ix.networking.portClaims has same-namespace port collisions:
            ${lib.concatMapAttrsStringSep "\n  " renderPortClaimConflict conflictingPortClaimGroups}

          Put services that need the same public protocol port in separate fleet nodes/VMs, or choose an explicit alternate port when same-image co-location is intentional.
        '';
      }
      {
        assertion = ipv4GuestHealthChecks == {};
        message = ''
          ix.healthChecks can only set requiresIpv4 on host checks:
            ${lib.concatStringsSep ", " (lib.attrNames ipv4GuestHealthChecks)}
        '';
      }
      {
        assertion = multiSugarHealthChecks == {};
        message = ''
          ix.healthChecks set more than one of `unit`, `http`, and `tcp`, which
          conflict (each derives the check's command):
            ${
            lib.concatMapAttrsStringSep ", " (
              name: check: "${name} (${lib.concatStringsSep " + " (probeSugars check)})"
            )
            multiSugarHealthChecks
          }

          Pick the one probe that proves readiness, or write an explicit
          `command` when a single probe is not enough.
        '';
      }
      {
        assertion = overSpecifiedHealthChecks == {};
        message = ''
          ix.healthChecks set both a probe sugar (`unit`, `http`, or `tcp`) and
          a custom `command`, which conflict (a custom command makes the sugar
          a no-op):
            ${lib.concatStringsSep ", " (lib.attrNames overSpecifiedHealthChecks)}

          Set `unit` for a `systemctl is-active` probe, `http`/`tcp` for a
          readiness probe, or `command` for an explicit argv -- not both.
        '';
      }
      {
        assertion = hostProbeSugarHealthChecks == {};
        message = ''
          ix.healthChecks can only use `http`/`tcp` probes on guest checks
          (their probe binaries are pinned inside the image, not on the
          operator's machine):
            ${lib.concatStringsSep ", " (lib.attrNames hostProbeSugarHealthChecks)}

          Keep `from = "guest"`, or write an explicit host `command` using
          tools from the operator's PATH.
        '';
      }
    ];

    # Keep declared `http`/`tcp` probe binaries in the image closure (and on
    # PATH for interactive debugging): the fleet plan strips string context
    # from check argv, so nothing else would retain them.
    environment.systemPackages = healthProbePackages;

    # The host platform (x86_64-linux) and the YourKit-only unfree predicate
    # live on the shared `imagePkgs` instantiation in `default.nix`: every
    # image shares ONE nixpkgs instance via `nixpkgs.pkgs`, and the nixpkgs
    # module ignores `hostPlatform`/`config` once `pkgs` is set.

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

      # ix-vm-guest writes the runtime-provided nameservers to /etc/resolv.conf
      # before starting the image. NixOS 26.05 enables resolvconf by default;
      # with no NixOS-owned nameservers, stage 2 then replaces that file with an
      # empty one and breaks DNS in an otherwise connected VM.
      resolvconf.enable = lib.mkDefault false;

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
        allowedTCPPorts =
          [
            5001 # ix-console shell and terminal snapshot listener.
          ]
          ++ exposeFirewallPorts "tcp";
        allowedUDPPorts =
          [
            8443 # ix-agent WebTransport direct-connect endpoint.
          ]
          ++ exposeFirewallPorts "udp";
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
      # The system flake registry pin for `nixpkgs` lives in
      # lib/image/default.nix (an inline module next to `nixpkgs.pkgs`),
      # because locking it needs the flake input's `narHash`, which only
      # that scope has.
      #
      # NIX_PATH so legacy nix-shell / <nixpkgs> imports resolve without a
      # channel subscription or network fetch.
      nixPath = ["nixpkgs=${pkgs.path}"];
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
        # nixpkgs' nix-daemon module bakes `sandbox-fallback = false` as a
        # "legacy configuration conversion" (nixos/modules/services/system/
        # nix-daemon.nix), which turns a build FATAL the moment the kernel
        # lacks the namespaces sandboxing needs. ix guests deliberately run
        # without user namespaces (hardening sets `allowNamespaces = false`)
        # and the VM is itself the isolation boundary, so a build that cannot
        # be sandboxed should degrade to an unsandboxed build with a warning,
        # not kill `ix up`. Restore Nix's own upstream default of `true`.
        # mkForce because the nixpkgs assignment is unconditional, not a
        # default. See indexable-inc/index#2453.
        # astlog-ignore: no-mkforce nixpkgs sets this unconditionally; #2453 owns the fix.
        sandbox-fallback = lib.mkForce true;
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
