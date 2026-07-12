# Always-on headless aarch64-linux builder VM for a darwin host, run as a
# native Apple Virtualization.framework guest via vfkit (no QEMU, no GPU, no
# desktop; we just ssh in). VZ sidesteps the M5/macOS-26 HVF SME bug that
# aborts a vCPU in mainline QEMU 11 (hvf_arch_init_vcpu HV_SYS_REG_SMCR_EL1
# assertion, an unfixed HVF/SME bug), which is also why nix.linux-builder
# (QEMU+HVF) is not an option on affected hosts.
#
# The guest is an ordinary self-booting NixOS appliance (pair with
# `nixosModules.builder-vm`): vfkit's EFI firmware boots systemd-boot from the
# ESP of ONE persistent GPT disk that owns the whole system: store, Nix
# database, generations, host keys. This module only supplies virtual
# hardware, so a darwin switch never rebuilds the guest. First install writes
# the repart-built image (`vm-install`); every later change is a normal
# remote NixOS deploy (`vm-deploy`).
#
# Closure boundary: the runner (`vm`) and `vm-ssh` land in systemPackages and
# reference only the state dir, never the guest closure, so the darwin system
# closure carries no aarch64-linux paths and a switch never waits on a Linux
# builder. `vm-install` (which embeds the guest image) and `vm-deploy` (which
# embeds the deploy flake ref) exist only behind the read-only `packages`
# option for the consuming flake to expose as `nix run` targets.
#
# `writeBashApplication` is injected at import time (flake.nix darwinModules)
# because `ix` is not in nix-darwin module scope; same pattern as
# modules/darwin/mutable-files.nix.
{writeBashApplication}: {
  config,
  lib,
  pkgs,
  ...
}: let
  inherit (lib) mkDefault mkEnableOption mkIf mkOption types;

  cfg = config.services.builder-vm;

  vfkitArgs = lib.escapeShellArgs [
    "--cpus"
    (toString cfg.cpus)
    "--memory"
    (toString cfg.memMiB)
    # EFI firmware with NVRAM persisted beside the disk (`create` makes it on
    # first run); systemd-boot on the guest's ESP takes it from there.
    "--bootloader"
    "efi,variable-store=efi-variable-store,create"
    "--device"
    "virtio-blk,path=root.img"
    "--device"
    "virtio-net,nat,mac=${cfg.mac}"
    "--device"
    "virtio-serial,stdio"
    "--device"
    "virtio-rng"
  ];

  # Boot the headless guest with vfkit. A pure virtual-hardware runner: it
  # references only the state dir provisioned by `vm-install`, never the
  # guest closure. Normally started by the launchd daemon below; run by hand
  # for an interactive serial console. Stop cleanly over the REST socket:
  #   curl --unix-socket "$stateDir/vm.sock" -XPOST \
  #     -d '{"state":"Stop"}' http://vfkit/vm/state
  vm = writeBashApplication pkgs {
    name = "vm";
    # coreutils so install is self-contained (writeBashApplication only
    # prepends runtimeInputs to PATH; don't depend on the caller's ambient
    # PATH). stty/script are the system ones by design.
    runtimeInputs = [
      pkgs.coreutils
      pkgs.flock
      pkgs.vfkit
    ];
    text = ''
      umask 077
      stateDir="''${VM_STATE_DIR:-${cfg.stateDir}}"
      install -d -m 0700 "$stateDir"
      cd "$stateDir"
      exec 9>vm.lock
      if ! flock -n 9; then
        echo "vm: another runner owns $stateDir" >&2
        exit 1
      fi
      if [ ! -e root.img ]; then
        echo "vm: no root.img in $stateDir; provision it with: vm-install" >&2
        exit 1
      fi
      chmod 0600 root.img
      rm -f vm.sock
      restfulUri="unix://$stateDir/vm.sock"
      if [ -t 0 ]; then
        # Interactive: rebind Ctrl-] to the tty signals so Ctrl-C reaches
        # the guest instead of killing the VM.
        save=$(stty -g)
        # shellcheck disable=SC2064
        trap "stty $save" EXIT
        stty intr ^] susp ^] quit ^]
        exec vfkit ${vfkitArgs} --restful-uri "$restfulUri"
      else
        # Headless (launchd): vfkit's virtio-serial,stdio console needs a
        # tty; launchd gives none, so a bare exec dies with "operation not
        # supported by device". Allocate a pty with `script`; the guest
        # console still flows to the daemon's StandardOutPath.
        exec /usr/bin/script -q /dev/null vfkit ${vfkitArgs} --restful-uri "$restfulUri"
      fi
    '';
  };

  # `vm-install [--reinstall]`: provision the disk from the built image. The
  # ONLY package that references the guest closure, so only installs (never
  # routine darwin switches) wait for an aarch64-linux image build; bootstrap
  # with an ad hoc builder if no guest is running yet, e.g.
  #   --builders 'ssh-ng://root@<ip>?ssh-key=<sshKey> aarch64-linux'
  vmInstall = writeBashApplication pkgs {
    name = "vm-install";
    runtimeInputs = [pkgs.coreutils];
    text = ''
      stateDir="''${VM_STATE_DIR:-${cfg.stateDir}}"
      if [ -e "$stateDir/root.img" ] && [ "''${1:-}" != "--reinstall" ]; then
        echo "vm-install: $stateDir/root.img already exists; pass --reinstall to DESTROY it (store, home, host keys)" >&2
        exit 1
      fi
      umask 077
      install -d -m 0700 "$stateDir"
      rm -f "$stateDir/root.img" "$stateDir/efi-variable-store"
      # Copy then grow: the image ships minimized; the guest's initrd
      # repart + growfs (nixosModules.builder-vm) claim the rest of the disk
      # on boot.
      cp ${cfg.guest.image}/${cfg.guest.imageFileName} "$stateDir/root.img.tmp"
      chmod 0600 "$stateDir/root.img.tmp"
      truncate -s ${toString cfg.diskGiB}G "$stateDir/root.img.tmp"
      mv "$stateDir/root.img.tmp" "$stateDir/root.img"
      echo "vm-install: installed ${cfg.guest.imageFileName} into $stateDir/root.img (${toString cfg.diskGiB}G); boot with the launchd daemon or 'vm'"
    '';
  };

  # `vm-deploy`: standard remote NixOS deploy of the guest. Evaluation and
  # builds happen on the darwin host (the aarch64-linux drvs dispatch to the
  # guest through the remote-builder record); only the closure copy and
  # activation go over ssh, as root via a regular authorized key (the builder
  # key is restricted to nix-daemon).
  vmDeploy = writeBashApplication pkgs {
    name = "vm-deploy";
    runtimeInputs = [pkgs.nixos-rebuild-ng];
    text = ''
      connect=${lib.getExe vmNetConnect}
      export NIX_SSHOPTS="-o ProxyCommand=$connect -o StrictHostKeyChecking=accept-new"
      exec nixos-rebuild switch --flake ${cfg.deploy.flake} --target-host root@vm "$@"
    '';
  };

  # `vm-net-connect`: stdin/stdout pipe to the running guest's ssh port, used
  # as an ssh ProxyCommand by both `vm-ssh` (below) and the host's
  # aarch64-linux build machine (the `remoteBuilder` record). It is the ONE
  # place that knows how to reach the guest, so the resolve + route logic
  # lives here, not duplicated per consumer.
  #
  # vfkit's `user` NAT puts the guest on the 192.168.64.0/24 family (host
  # index not guaranteed; another VZ guest may take a lower index), reachable
  # from the host but absent from /var/db/dhcpd_leases. Resolve its IP +
  # bridge from the host ARP table by the guest's unique MAC, warming ARP with
  # a subnet sweep when cold and retrying a few rounds, so the nix-daemon
  # build path self-heals on a cold or just-rebooted guest without its own
  # wrapper.
  #
  # Then re-assert the interface-scoped connected route on every connection:
  # Tailscale (accept-routes) strips the vmnet /24 and pulls the guest subnet
  # into the tunnel, so a bare connect gets "Network is unreachable". A
  # longest-prefix /24 via the guest's bridge beats Tailscale's broad route.
  # Needs root: the nix-daemon ProxyCommand already runs as root; the human
  # path (vm-ssh) relies on the host's NOPASSWD sudo. arp/ping/route/nc/sudo
  # are the SYSTEM binaries by absolute path on purpose: nixpkgs tools don't
  # honor macOS's scoped vmnet route, while Apple's do.
  vmNetConnect = writeBashApplication pkgs {
    name = "vm-net-connect";
    # gawk for strtonum (MAC normalize; macOS awk lacks it); coreutils so
    # seq/id are self-contained (writeBashApplication only prepends
    # runtimeInputs to PATH).
    runtimeInputs = [
      pkgs.gawk
      pkgs.coreutils
    ];
    text = ''
      mac="${cfg.mac}"
      # macOS `arp` prints MAC octets without a leading zero (0c -> c), so a
      # literal substring match misses. Normalize both sides to 2-hex-digit
      # octets before comparing (robust for any MAC). Prints "IP BRIDGE".
      find_addr() {
        /usr/sbin/arp -an 2>/dev/null | awk -v target="$mac" '
          function norm(s,   a, n, i, o) {
            n = split(s, a, ":"); o = "";
            for (i = 1; i <= n; i++) o = o (i > 1 ? ":" : "") sprintf("%02x", strtonum("0x" a[i]));
            return o;
          }
          { m = ""; iface = "";
            for (i = 1; i <= NF; i++) { if ($i == "at") m = $(i + 1); if ($i == "on") iface = $(i + 1); }
            if (m != "" && norm(m) == norm(target)) { ip = $2; gsub(/[()]/, "", ip); print ip, iface; exit }
          }'
      }
      # Resolve with retries: ARP is empty until the guest answers traffic,
      # so on a cold or just-rebooted guest the first lookup misses. Each
      # round sweeps the low host range of the vmnet bridge subnets to warm
      # ARP (all pings backgrounded, ~1s) then re-checks. This is what lets
      # the nix-daemon build path connect without its own retry loop.
      addr=""
      for _ in 1 2 3 4 5; do
        addr=$(find_addr)
        [ -n "$addr" ] && break
        for n in $(seq 64 96); do
          for h in $(seq 2 16); do /sbin/ping -c1 -t1 "192.168.$n.$h" >/dev/null 2>&1 & done
        done
        wait
        addr=$(find_addr)
        [ -n "$addr" ] && break
        sleep 1
      done
      if [ -z "$addr" ]; then
        echo "vm-net-connect: VM MAC $mac not in ARP, is it up? (vm / launchd daemon)" >&2
        exit 1
      fi
      ip=''${addr%% *}
      iface=''${addr##* }
      net="''${ip%.*}.0/24"
      if [ "$(id -u)" = 0 ]; then
        /sbin/route -n add -net "$net" -interface "$iface" >/dev/null 2>&1 || true
      else
        /usr/bin/sudo -n /sbin/route -n add -net "$net" -interface "$iface" >/dev/null 2>&1 || true
      fi
      exec /usr/bin/nc "$ip" 22
    '';
  };

  # `vm-ssh [cmd...]`: ssh into the running guest. Reaches it through
  # vm-net-connect (resolve + route), retrying on ssh's connection-failure
  # code (255) only (any other exit means we connected and ran, so pass it
  # through). The host key persists on the guest's disk, so accept-new pins
  # it on the first successful connection.
  vmSsh = writeBashApplication pkgs {
    name = "vm-ssh";
    runtimeInputs = [pkgs.coreutils]; # seq/sleep
    text = ''
      connect=${lib.getExe vmNetConnect}
      rc=255
      for _ in $(seq 1 8); do
        # `|| rc=$?` is required: writeBashApplication runs `set -e`, so a
        # bare failing ssh would abort the script before we could retry.
        rc=0
        /usr/bin/ssh \
          -o ProxyCommand="$connect" \
          -o StrictHostKeyChecking=accept-new \
          -o LogLevel=ERROR \
          -o ConnectTimeout=8 \
          ${cfg.sshUser}@vm "$@" || rc=$?
        [ "$rc" -ne 255 ] && exit "$rc"
        sleep 2
      done
      echo "vm-ssh: could not reach the VM after retries" >&2
      exit "$rc"
    '';
  };
in {
  options.services.builder-vm = {
    enable = mkEnableOption "the always-on headless vfkit builder VM daemon";

    mac = mkOption {
      type = types.str;
      example = "5a:94:ef:2b:17:0c";
      description = ''
        Guest MAC address, the handle `vm-net-connect` resolves against the
        host ARP table. Pick one fixed value so the lookup is stable, and
        unique per guest so two VMs never collide while both exist.
      '';
    };

    cpus = mkOption {
      type = types.ints.positive;
      description = ''
        Guest vCPUs. Size for real aarch64-linux builds: an undersized guest
        leaves a many-core host mostly idle while multi-hour closures crawl.
      '';
    };

    memMiB = mkOption {
      type = types.ints.positive;
      description = ''
        Guest memory in MiB. RAM matters as much as cores because LLVM link
        steps are memory-bound.
      '';
    };

    diskGiB = mkOption {
      type = types.ints.positive;
      description = ''
        Virtual-disk capacity. The image ships minimized; `vm-install`
        truncates the installed copy up to this size and the guest's initrd
        repart + growfs claim the space on boot (nixosModules.builder-vm).
      '';
    };

    stateDir = mkOption {
      type = types.str;
      default = "/var/lib/vm-builder";
      description = ''
        Guest state directory, owned by the launchd daemon; the runner honors
        `VM_STATE_DIR` for ad hoc instances. Holds the guest's whole world:
        root.img, EFI variable store, vm.sock (REST control), vm.lock.
      '';
    };

    sshKey = mkOption {
      type = types.str;
      default = "/etc/nix/builder_ed25519";
      description = ''
        Private half of the dedicated builder key the nix-daemon uses to
        reach the guest. Root-owned, OUTSIDE the Nix store and git; the guest
        authorizes the public half restricted to `nix-daemon --stdio`
        (nixosModules.builder-vm `builderPublicKey`).
      '';
    };

    sshUser = mkOption {
      type = types.str;
      default = "root";
      description = "Login `vm-ssh` connects as.";
    };

    logFile = mkOption {
      type = types.str;
      default = "/var/log/vm-builder.log";
      description = "Launchd daemon log; also carries the guest serial console.";
    };

    guest = {
      image = mkOption {
        type = types.nullOr types.package;
        default = null;
        description = ''
          The guest's repart-built disk image
          (`nixosConfigurations.<guest>.config.system.build.image`). Only
          `vm-install` references it, so leaving it null just drops
          `vm-install` from `packages`; nothing in the darwin system closure
          ever depends on it.
        '';
      };
      imageFileName = mkOption {
        type = types.nullOr types.str;
        default = null;
        example = "vm.raw";
        description = ''
          Image file name inside `guest.image`
          (`nixosConfigurations.<guest>.config.image.filePath`).
        '';
      };
    };

    deploy.flake = mkOption {
      type = types.nullOr types.str;
      default = null;
      example = "github:example/config#vm";
      description = ''
        Flake ref (installable) of the guest's nixosConfiguration for
        `vm-deploy`. Leaving it null drops `vm-deploy` from `packages`.
      '';
    };

    maxJobs = mkOption {
      type = types.ints.positive;
      defaultText = lib.literalExpression "config.services.builder-vm.cpus";
      description = "Build slots advertised on the remote-builder record: one job per guest vCPU.";
    };

    speedFactor = mkOption {
      type = types.ints.positive;
      default = 1;
      description = "Relative speed on the remote-builder record.";
    };

    supportedFeatures = mkOption {
      type = types.listOf types.str;
      default = [
        "big-parallel"
        "benchmark"
        "ca-derivations"
      ];
      description = ''
        Features advertised on the remote-builder record. `kvm` and
        `nixos-test` are deliberately NOT in the default: the vfkit guest has
        no /dev/kvm, unlike a real Linux host.
      '';
    };

    packages = mkOption {
      type = types.attrsOf types.package;
      readOnly = true;
      description = ''
        The host tools, for the consuming flake to expose as packages: `vm`,
        `vm-ssh`, `vm-net-connect`, plus `vm-install` when `guest.image` is
        set and `vm-deploy` when `deploy.flake` is set.
      '';
    };

    remoteBuilder = mkOption {
      type = types.raw;
      readOnly = true;
      description = ''
        This guest as a `nix.remoteBuilders` entry
        (darwinModules.remote-builders); also consumable by hand-rolled
        `nix.buildMachines` wiring.
      '';
    };
  };

  config = {
    services.builder-vm = {
      maxJobs = mkDefault cfg.cpus;

      packages =
        {
          inherit vm;
          vm-ssh = vmSsh;
          vm-net-connect = vmNetConnect;
        }
        // lib.optionalAttrs (cfg.guest.image != null) {vm-install = vmInstall;}
        // lib.optionalAttrs (cfg.deploy.flake != null) {vm-deploy = vmDeploy;};

      remoteBuilder = {
        # Keep this alias distinct from any imperative `Host vm` ssh entry.
        # OpenSSH uses the first value it reads, and such an entry can enable
        # connection multiplexing, which corrupts parallel nix ssh-ng
        # protocol streams.
        name = "vm-builder";
        hostName = "vm";
        user = "root";
        inherit
          (cfg)
          sshKey
          maxJobs
          speedFactor
          supportedFeatures
          ;
        systems = ["aarch64-linux"];
        ssh = {
          # The guest's host keys persist on its disk; accept-new pins them
          # on the first successful connection.
          strictHostKeyChecking = "accept-new";
          proxyCommand = lib.getExe vmNetConnect;
        };
      };
    };

    # The runner is pure virtual hardware: it never references the guest
    # closure, so darwin switches don't need a Linux builder (only
    # `vm-install` builds the guest image).
    environment.systemPackages = mkIf cfg.enable [
      vm
      vmSsh
    ];

    # Always-on headless guest VM via vfkit. Runs as root so vfkit's vmnet
    # NAT works without an app entitlement; the state directory holds the
    # guest's whole world (see `stateDir`). Provision it once with
    # `vm-install`. KeepAlive reboots the guest if it powers off;
    # ThrottleInterval keeps a crash-looping boot from spinning. ssh in with
    # `vm-ssh`.
    launchd.daemons.vm-builder = mkIf cfg.enable {
      serviceConfig = {
        Label = "org.nixos.vm-builder";
        ProgramArguments = [(lib.getExe vm)];
        EnvironmentVariables.VM_STATE_DIR = cfg.stateDir;
        KeepAlive = true;
        RunAtLoad = true;
        ThrottleInterval = 10;
        ProcessType = "Background";
        StandardOutPath = cfg.logFile;
        StandardErrorPath = cfg.logFile;
      };
    };
  };
}
