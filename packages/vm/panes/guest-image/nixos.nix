# aarch64 NixOS guest for the panes seamless-windows system (index#1686):
# boots headless under `vmkit boot-linux --gpu` (libkrun-efi), runs the
# `panes-compositor` Wayland compositor (exports each toplevel to the macOS
# host over vsock port 7100, panes-protocol VSOCK_PORT), and starts one
# systemd-nspawn container per app from ./apps.nix, each a Wayland client of
# that compositor.
#
# The disk is assembled with **systemd-repart** (via the image/repart module),
# NOT `make-disk-image`: repart runs in the build sandbox with no qemu/kvm VM,
# so the image builds on a plain aarch64-linux builder. libkrun-efi boots
# OVMF -> systemd-boot (at the EFI removable path) -> the UKI in /EFI/Linux.
# Modeled on packages/vm/chrome-vm-image.
{
  lib,
  pkgs,
  config,
  modulesPath,
  ...
}:
let
  apps = import ./apps.nix { inherit pkgs; };

  # Where the compositor's Wayland socket lives on the host and, bind-mounted,
  # inside every app container. Deliberately not a systemd RuntimeDirectory:
  # that is torn down when the compositor stops, yanking the bind source out
  # from under running containers; a tmpfiles.d dir survives restarts.
  runtimeDir = "/run/panes";
  waylandDisplay = "wayland-1";

  # Environment every Wayland client container gets; per-app env from apps.nix
  # is merged on top and wins.
  clientEnv = {
    WAYLAND_DISPLAY = waylandDisplay;
    XDG_RUNTIME_DIR = runtimeDir;
  };

  # Host paths apps want persisted (e.g. /var/lib/minecraft downloads); the
  # host creates them via tmpfiles so the container bind mounts do not fail on
  # a missing source.
  appBinds = lib.unique (lib.concatMap (app: app.binds or [ ]) (lib.attrValues apps));

  # Per-app seed files (apps.nix `files`), flattened to one host path -> source
  # map. Rendered as tmpfiles `C` rules: copy only when the destination does
  # not exist yet, and 0644 (not the store's 0444) so the app can rewrite its
  # own config afterwards (MC persists Video Settings back to options.txt).
  appSeedFiles = lib.mergeAttrsList (map (app: app.files or { }) (lib.attrValues apps));

  # Render one apps.nix entry into a declarative systemd-nspawn container.
  # The container shares the host network namespace (default, no
  # privateNetwork: portablemc needs outbound net through gvproxy) and the
  # host /nix/store, and sees the compositor socket + the venus render node
  # via bind mounts.
  mkAppContainer = name: app: {
    autoStart = true;
    bindMounts = {
      ${runtimeDir} = {
        hostPath = runtimeDir;
        isReadOnly = false;
      };
    }
    # Only GPU apps bind /dev/dri: nspawn fails the whole container when a
    # bind source is missing, and the guest may boot GPU-less (no --gpu, or
    # the venus stack is down); shm apps must keep working then.
    // lib.optionalAttrs (app.gpu or false) {
      "/dev/dri" = {
        hostPath = "/dev/dri";
        isReadOnly = false;
      };
    }
    // lib.genAttrs (app.binds or [ ]) (bind: {
      hostPath = bind;
      isReadOnly = false;
    });
    # The bind mount alone is not enough: nspawn's device cgroup policy still
    # denies the node unless whitelisted here. venus/zink renders on the
    # virtio-gpu render node.
    allowedDevices = lib.optional (app.gpu or false) {
      node = "/dev/dri/renderD128";
      modifier = "rwm";
    };
    config = {
      # /run/opengl-driver inside the container: mesa's venus vulkan ICD plus
      # the zink GL driver for clients that go through the loader.
      hardware.graphics.enable = true;
      systemd.services."panes-app-${name}" = {
        description = "panes app: ${name}";
        wantedBy = [ "multi-user.target" ];
        environment = clientEnv // (app.env or { });
        serviceConfig = {
          ExecStart = app.command;
          # The compositor may not be listening yet (or is mid-restart);
          # retrying is what makes the window appear once it is.
          Restart = "on-failure";
          RestartSec = 2;
        };
      };
      system.stateVersion = "24.11";
    };
  };
in
{
  imports = [ "${modulesPath}/image/repart.nix" ];

  # Boot path: OVMF (libkrun-efi) -> systemd-boot (EFI removable path) -> UKI.
  # The bootloader + UKI are placed manually via repart, so disable grub.
  boot = {
    loader.grub.enable = false;
    # Grow the root partition to fill the (host-enlarged) disk at boot; with
    # the scripted initrd this is cloud-utils growpart, then autoResize's
    # resize2fs. Pairs with the minimized repart root below.
    growPartition = true;
    # hvc0 is the libkrun serial console vmkit streams/captures; unlike
    # chrome-vm-image we keep kernel printk on it, boot logs are the point of
    # the smoke test.
    kernelParams = [ "console=hvc0" ];
    initrd.availableKernelModules = [
      "virtio_pci"
      "virtio_blk"
      "virtio_console"
      "sd_mod"
    ];
    kernelModules = [
      # Creates /dev/dri/renderD128 for venus (vmkit boot-linux --gpu).
      "virtio_gpu"
      # Guest side of the vsock the compositor listens on (port 7100).
      "vmw_vsock_virtio_transport"
    ];
    # 16 KiB guest pages, required for venus blob mapping under libkrun on
    # Apple Silicon. The host maps each RESOURCE_MAP_BLOB with hv_vm_map,
    # which rejects any address/size not 16 KiB-aligned (the fixed macOS host
    # page size; HV_BAD_ARGUMENT, verified empirically). The guest kernel
    # PAGE_ALIGNs every blob size (virtgpu_vram.c) and packs blob offsets in
    # the host-visible window at that same granularity, so with default 4K
    # pages the very first venus allocation (the 0x21000-byte instance ring
    # shmem) reaches hv_vm_map 4K-aligned only and fails: the guest sees
    # ERR_UNSPEC on MAP_BLOB/UNMAP_BLOB (0x208/0x209) and mesa falls back to
    # lavapipe. 16K pages make every blob size and offset a 16K multiple.
    # Same configuration as muvm/Asahi, libkrun's reference GPU guests.
    kernelPatches = [
      {
        name = "arm64-16k-pages";
        patch = null;
        structuredExtraConfig.ARM64_16K_PAGES = lib.kernel.yes;
      }
    ];
  };

  # Root is the repart "root"-typed partition, found by its GPT partition label.
  # autoResize pairs with boot.growPartition: the shipped image is minimized
  # (see the repart config below), and boot expands root into whatever extra
  # space the host gave the disk file.
  fileSystems."/" = {
    device = "/dev/disk/by-partlabel/root";
    fsType = "ext4";
    autoResize = true;
  };

  system.image = {
    id = "panes-guest";
    version = "1";
  };

  image.repart = {
    name = "panes-guest";
    # OVMF does not work with repart's default 4096-byte sector size.
    sectorSize = 512;
    partitions = {
      "esp" = {
        contents =
          let
            # aarch64-only image (see package.nix), so the EFI arch is fixed.
            # Avoids depending on `config.nixpkgs.hostPlatform` (unset under
            # eval-config with a bare `system`).
            efiArch = "aa64";
          in
          {
            "/EFI/BOOT/BOOT${lib.toUpper efiArch}.EFI".source =
              "${pkgs.systemd}/lib/systemd/boot/efi/systemd-boot${efiArch}.efi";
            "/EFI/Linux/${config.system.boot.loader.ukiFile}".source =
              "${config.system.build.uki}/${config.system.boot.loader.ukiFile}";
            # Auto-boot the single UKI with no menu/delay.
            "/loader/loader.conf".source = pkgs.writeText "loader.conf" "timeout 0\n";
          };
        repartConfig = {
          Type = "esp";
          Format = "vfat";
          # Roomy for the UKI (kernel + initrd) on aarch64.
          SizeMinBytes = "256M";
        };
      };
      "root" = {
        storePaths = [ config.system.build.toplevel ];
        repartConfig = {
          Type = "root";
          Format = "ext4";
          Label = "root";
          # Minimize, do NOT bake runtime headroom (the issue's 8-12GiB note)
          # into the partition: repart stages the populated ext4 as a temp
          # file and then copies it into the final raw, so a fixed N-GiB root
          # costs ~2N on the builder disk at once, which overflows the
          # aarch64 builder VM ("Failed to copy bytes to partition: No space
          # left on device"). Runtime free space (portablemc downloads into
          # /var/lib/minecraft) comes from growing instead: enlarge the disk
          # file before boot (truncate -s 8G a copy of the image) and
          # growPartition + autoResize below expand root into it.
          Minimize = "guess";
        };
      };
    };
  };

  # gvproxy (vmkit --net) puts the guest on 192.168.127.0/24 with DHCP from
  # the .1 gateway; without DHCP the guest has no route out.
  networking.useDHCP = true;

  # Root autologin on the serial console: this guest is a local dev appliance
  # reachable only through hvc0 (no ssh), and headless debugging (venus state,
  # container journals, poking the MC launcher) needs a shell there.
  services.getty.autologinUser = "root";

  # Populates /run/opengl-driver (lib + share/vulkan/icd.d) with mesa, which
  # carries the venus ICD (virtio_icd.aarch64.json) on this nixpkgs pin; the
  # patched vulkan-loader looks there.
  hardware.graphics.enable = true;

  # 0777: the compositor (root on the host) creates the socket here and app
  # processes (container root) connect through the bind mount; wide perms keep
  # the v1 single-user guest simple.
  systemd.tmpfiles.rules = [
    "d ${runtimeDir} 0777 root root -"
    # The repart root ships /nix/store contents (storePaths) but no nix
    # database; nixos-containers' nspawn unit bind-mounts these two read-only
    # and a missing bind source fails the whole container. Empty dirs satisfy
    # the binds (nothing runs nix inside the containers).
    "d /nix/var/nix/db 0755 root root -"
    "d /nix/var/nix/daemon-socket 0755 root root -"
  ]
  ++ map (bind: "d ${bind} 0755 root root -") appBinds
  ++ lib.mapAttrsToList (dest: source: "C ${dest} 0644 root root - ${source}") appSeedFiles;

  systemd.services = {
    panes-compositor = {
      description = "panes guest compositor (Wayland toplevels -> vsock 7100)";
      wantedBy = [ "multi-user.target" ];
      # The socket dir is a tmpfiles.d entry (see above), not a
      # RuntimeDirectory; order after tmpfiles so it exists on first start.
      after = [ "systemd-tmpfiles-setup.service" ];
      environment = clientEnv // {
        # The compositor's `gpu` readback path dlopens `libEGL.so.1` at
        # runtime (smithay's `backend_egl` via libloading; deliberately no
        # link-time GL dep, so no rpath to resolve it). That soname is
        # libglvnd's dispatcher, which nixpkgs compiles with
        # /run/opengl-driver/share/glvnd/egl_vendor.d as its vendor-config
        # default and mesa's vendor JSON points at the store absolutely, so
        # the dispatcher alone is enough to land in the venus EGL driver
        # hardware.graphics provides.
        LD_LIBRARY_PATH = lib.makeLibraryPath [ pkgs.libglvnd ];
      };
      serviceConfig = {
        ExecStart = lib.getExe pkgs.panes-compositor;
        Restart = "on-failure";
        RestartSec = 5;
        # Mirror to the serial console: `vmkit boot-linux` captures hvc0 and
        # the boot smoke reads service state off it.
        StandardOutput = "journal+console";
        StandardError = "journal+console";
      };
    };

    # Boot-time diagnostic kept on purpose: prints the Vulkan device list to
    # the serial console so a headless `vmkit boot-linux --gpu` smoke run can
    # assert the venus path end to end. With --gpu it must show a
    # "Virtio-GPU Venus" deviceName; lavapipe/llvmpipe only means the guest
    # fell back to software (see packages/vm/vmkit/docs/linux-libkrun.md).
    panes-venus-smoke = {
      description = "Log Vulkan devices (expect Virtio-GPU Venus) to the serial console";
      wantedBy = [ "multi-user.target" ];
      after = [ "multi-user.target" ];
      path = [
        pkgs.vulkan-tools
        pkgs.gnugrep
      ];
      serviceConfig = {
        Type = "oneshot";
        StandardOutput = "journal+console";
        StandardError = "journal+console";
        TimeoutStartSec = 120;
      };
      script = ''
        echo "===PANES-VENUS-SMOKE-BEGIN==="
        out=$(vulkaninfo --summary 2>&1 || true)
        if printf '%s' "$out" | grep -qi venus; then
          printf '%s\n' "$out" | grep -iE "venus|deviceName|driverName"
        else
          echo "PANES-VENUS-ABSENT"
          # The full loader output is the diagnostic when venus is missing
          # (no ICD, no render node, capset mismatch, ...).
          printf '%s\n' "$out" | head -40
        fi
        ls -la /dev/dri 2>&1 || true
        echo "===PANES-VENUS-SMOKE-END==="
      '';
    };

    # Second boot-time diagnostic: dump why anything failed (the app
    # containers especially) to the serial console, since a headless
    # `vmkit boot-linux` run has no shell to poke around with.
    panes-boot-report = {
      description = "Log failed units and container journals to the serial console";
      wantedBy = [ "multi-user.target" ];
      after = [ "multi-user.target" ];
      path = [
        pkgs.systemd
        pkgs.iproute2
      ];
      serviceConfig = {
        Type = "oneshot";
        StandardOutput = "journal+console";
        StandardError = "journal+console";
        TimeoutStartSec = 180;
      };
      script = ''
        # Give autoStart containers a moment to attempt their first start.
        sleep 10
        echo "===PANES-BOOT-REPORT-BEGIN==="
        systemctl --failed --no-legend --plain || true
        for unit in $(systemctl --failed --no-legend --plain | cut -d' ' -f1); do
          echo "--- journal: $unit ---"
          journalctl -u "$unit" -n 20 --no-pager || true
        done
        # DHCP evidence: gvproxy leases 192.168.127.2/24 with gateway .1.
        ip -4 addr show || true
        ip route show default || true
        ls -la /run/panes 2>&1 || true
        df -h / || true
        echo "===PANES-BOOT-REPORT-END==="
      '';
    };
  };

  containers = lib.mapAttrs mkAppContainer apps;

  # For live poking at the GPU stack over the serial console.
  environment.systemPackages = [
    pkgs.vulkan-tools
    pkgs.pciutils
  ];

  # Trim the image; the compositor is the UI, there is no doc consumer.
  documentation.enable = lib.mkDefault false;
  system.stateVersion = "24.11";
}
