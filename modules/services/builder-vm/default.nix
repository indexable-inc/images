# Headless aarch64 NixOS builder guest for a darwin host: the host's
# aarch64-linux remote builder (pair with `darwinModules.builder-vm`, which
# owns the vfkit runner, launchd daemon, and ARP-by-MAC ProxyCommand). It runs
# as a native Apple Virtualization.framework VM via vfkit (no QEMU, no GPU, no
# desktop; we just ssh in). VZ sidesteps the M5/macOS-26 HVF SME bug that
# aborts mainline QEMU (see modules/darwin/builder-vm.nix).
#
# The guest is an ordinary self-booting NixOS appliance: vfkit's EFI firmware
# boots systemd-boot from the ESP of ONE persistent GPT disk that owns the
# whole system: store, Nix database, generations, host keys. The host only
# supplies virtual hardware, so a darwin switch never rebuilds the guest.
# First install writes the repart-built image (`image.repart` below, the host
# module's `vm-install`); every later change is a normal remote NixOS deploy
# (the host module's `vm-deploy`).
#
# Deliberately appliance-only: user accounts, home-manager profiles,
# substituters, and access tokens are consumer policy and stay in the
# importing flake.
{
  config,
  lib,
  pkgs,
  modulesPath,
  ...
}: let
  inherit (lib) mkDefault mkEnableOption mkIf mkOption types;

  cfg = config.services.builder-vm;
  efiArch = config.nixpkgs.hostPlatform.efiArch;
in {
  imports = [
    # Provides `image.repart` (not in the default module list). Imported
    # unconditionally (module imports cannot depend on config); it only
    # defines options and a lazy `system.build.image`, so it stays inert for
    # every eval that leaves this module disabled.
    (modulesPath + "/image/repart.nix")
  ];

  options.services.builder-vm = {
    enable = mkEnableOption "the headless vfkit builder guest appliance";

    builderPublicKey = mkOption {
      type = types.str;
      description = ''
        Public half of the host's dedicated builder key (the private half is
        the host module's `sshKey`). Authorized for root, restricted to the
        Nix daemon protocol.
      '';
    };

    authorizedKeyFiles = mkOption {
      type = types.listOf types.path;
      default = [];
      description = ''
        Regular public keys authorized for root, for interactive recovery and
        `vm-deploy` activation (the builder key is restricted to nix-daemon).
      '';
    };
  };

  config = mkIf cfg.enable {
    # --- One GPT disk (virtio-blk), partitions found by GPT label, never by
    # device slot: a by-partlabel reference cannot silently rebind if the host
    # ever attaches another disk (the exact failure that once mkfs'd a data
    # volume under the old multi-image layout).
    fileSystems."/" = {
      device = "/dev/disk/by-partlabel/root";
      fsType = "ext4";
      # x-systemd.growfs: grow the filesystem into the partition grown by the
      # initrd repart service below.
      autoResize = true;
    };
    fileSystems."/boot" = {
      device = "/dev/disk/by-partlabel/esp";
      fsType = "vfat";
      options = ["umask=0077"];
    };

    boot = {
      # Serial console on vfkit's virtio-serial,stdio (which the host module's
      # launchd daemon writes to its log file).
      kernelParams = ["console=hvc0"];
      initrd = {
        availableKernelModules = [
          "virtio_pci"
          "virtio_blk"
        ];
        systemd = {
          enable = true;
          # vm-install grows the installed image to the minimum virtual-disk
          # size; claim that space on boot: repart grows the root *partition*
          # in the initrd, autoResize (above) grows the *filesystem* into it.
          # Both are no-ops once grown, so this stays enabled.
          repart = {
            enable = true;
            device = "/dev/vda";
          };
        };
      };

      loader.systemd-boot.enable = true;
      # Bound kernel+initrd copies on the ESP (sized 1G in image.repart below).
      loader.systemd-boot.configurationLimit = mkDefault 5;
    };

    # First boot of a freshly installed image: the image carries the system
    # closure but no Nix database and no bootloader-owned generation. Register
    # both, then hand the ESP to systemd-boot; the bootstrap UKI (image.repart
    # below) is deleted so generation entries own every later boot. A stage-2
    # unit rather than boot.postBootCommands: under systemd stage-1 those run
    # in initrd-nixos-activation, before the ESP is mounted, so the bootloader
    # install there fails check-mountpoints.
    systemd.services.first-boot-registration = {
      description = "Register the appliance image as the first NixOS generation";
      wantedBy = ["multi-user.target"];
      before = ["nix-daemon.service"];
      unitConfig = {
        ConditionPathExists = "/nix-path-registration";
        RequiresMountsFor = ["/boot"];
      };
      serviceConfig = {
        Type = "oneshot";
        RemainAfterExit = true;
      };
      path = [config.nix.package];
      script = ''
        nix-store --load-db < /nix-path-registration
        touch /etc/NIXOS
        nix-env -p /nix/var/nix/profiles/system --set /run/current-system
        NIXOS_INSTALL_BOOTLOADER=1 /run/current-system/bin/switch-to-configuration boot
        rm /boot/EFI/Linux/*.efi /nix-path-registration
      '';
    };
    # Definition for the initrd repart grow above: match the root partition by
    # type and let it take all remaining disk (repart's default upper bound).
    systemd.repart.partitions.root.Type = "root";

    # --- Install image (`system.build.image`, aarch64-linux): a GPT disk
    # assembled offline by systemd-repart (no VM or KVM needed on the builder,
    # so the running guest can build its own successor image). The ESP carries
    # a bootstrap systemd-boot + unified kernel image used only until the
    # first-boot service installs the real bootloader; the root partition
    # carries the closure plus the database registration it consumes. Shipped
    # minimized and grown at install/boot (see the host module's vm-install
    # and the repart service above).
    image.repart = {
      name = mkDefault "vm";
      # 512-byte sectors: what Virtualization.framework virtio-blk exposes.
      sectorSize = 512;
      partitions = {
        esp = {
          contents = {
            "/EFI/BOOT/BOOT${lib.toUpper efiArch}.EFI".source = "${pkgs.systemd}/lib/systemd/boot/efi/systemd-boot${efiArch}.efi";
            "/EFI/Linux/${config.system.boot.loader.ukiFile}".source = "${config.system.build.uki}/${config.system.boot.loader.ukiFile}";
          };
          repartConfig = {
            Type = "esp";
            Format = "vfat";
            Label = "esp";
            # Room for `configurationLimit` kernel+initrd generations.
            SizeMinBytes = "1G";
          };
        };
        root = {
          storePaths = [config.system.build.toplevel];
          contents."/nix-path-registration".source = "${
            pkgs.closureInfo {rootPaths = [config.system.build.toplevel];}
          }/registration";
          repartConfig = {
            Type = "root";
            Format = "ext4";
            Label = "root";
            Minimize = "guess";
          };
        };
      };
    };

    networking = {
      hostName = mkDefault "vm";
      useDHCP = mkDefault true;
      # vfkit's DHCP currently advertises 192.168.64.1 as DNS, but that stub
      # can refuse queries from the guest while the host resolves fine. Use
      # explicit upstream DNS so remote Nix builds can reach substituters.
      nameservers = mkDefault [
        "1.1.1.1"
        "1.0.0.1"
      ];
    };

    # Root accepts the host's dedicated builder key only for the Nix daemon
    # protocol (the host module's remote-builder record, reached via the
    # vm-net-connect ProxyCommand; the matching private key is the host
    # module's `sshKey`). Regular keys (`authorizedKeyFiles`) stay available
    # for interactive recovery and `vm-deploy` activation.
    users.users.root.openssh.authorizedKeys = {
      keyFiles = cfg.authorizedKeyFiles;
      keys = [
        ''restrict,command="${config.nix.package}/bin/nix-daemon --stdio" ${cfg.builderPublicKey}''
      ];
    };
    # Root autologin on the serial console (which lands in the host daemon's
    # log file) for headless debugging if ssh ever isn't reachable.
    services.getty.autologinUser = mkDefault "root";

    services.openssh = {
      enable = true;
      # Host keys live at the default /etc/ssh paths on the persistent root
      # disk; the host pins them via accept-new on first connection.
      settings = {
        PermitRootLogin = "prohibit-password";
        PasswordAuthentication = mkDefault false;
      };
    };

    nix.settings = {
      # This box exists to build; never fall back to unsandboxed builds.
      sandbox-fallback = false;
      # Collect only under storage pressure (nix.gc.automatic stays off,
      # below): periodic GC would discard the persistent builder cache even
      # when the disk has ample room.
      min-free = mkDefault (30 * 1024 * 1024 * 1024);
      max-free = mkDefault (60 * 1024 * 1024 * 1024);

      # launchd can terminate vfkit without a guest shutdown. Commit imported
      # paths to disk before registering them so an abrupt stop cannot leave
      # registered-but-unwritten store paths.
      sync-before-registering = true;

      # Download hardening, learned the hard way on a flaky uplink: a
      # black-holed cache connection otherwise stalls a multi-hour build
      # silently (DNS resolves, TCP never progresses). Fail fast, retry more.
      stalled-download-timeout = mkDefault 60;
      download-attempts = mkDefault 8;
    };
    nix.gc.automatic = mkDefault false;
  };
}
