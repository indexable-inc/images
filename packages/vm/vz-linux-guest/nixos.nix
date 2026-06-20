# NixOS aarch64 guest for the vmkit `boot-linux-gui` path: boots straight into
# a Wayland compositor on the virtio-gpu and runs `bossbar-overlay`, so the host
# can screenshot a real Linux GUI render off the VZ framebuffer.
#
# Software graphics only: Apple's virtio-gpu has no 3D acceleration, so the
# compositor uses the wlroots pixman renderer and the wgpu app uses Mesa
# lavapipe (software Vulkan). See the index `linux-gui-vm-vz` notes.
{
  lib,
  pkgs,
  ...
}:
let
  # The overlaid aarch64-linux package, injected through `nixpkgs.overlays` by
  # the builder (default.nix) rather than a specialArg.
  inherit (pkgs) bossbar-overlay;
  # Wrapper that points the wgpu overlay at the lavapipe software-Vulkan ICD
  # (mandatory: with no ICD wgpu hard-panics, there is no GL fallback) and seeds
  # a fresh BOSSBAR_DB path so the overlay auto-populates its demo bars.
  #
  # Kept as raw bash: this module is a standalone `eval-config.nix` system with
  # no package `ix` in scope, so the checked `ix.writeBashApplication` /
  # `ix.writeNushellApplication` writers are unreachable here. It is a trivial
  # env-setup + exec launch wrapper.
  # astlog-ignore: no-write-shell-script
  bossbarLaunch = pkgs.writeShellScript "bossbar-launch" ''
    export VK_DRIVER_FILES=${pkgs.mesa}/share/vulkan/icd.d/lvp_icd.aarch64.json
    export LD_LIBRARY_PATH=${pkgs.mesa}/lib
    export BOSSBAR_DB=/tmp/bossbars.db
    exec ${lib.getExe bossbar-overlay}
  '';

  # Minimal sway config: a fixed-size output and the overlay as the only client.
  swayConfig = pkgs.writeText "sway-config" ''
    output * resolution 1920x1080 position 0,0
    default_border none
    exec ${bossbarLaunch}
  '';
in
{
  boot = {
    # Boot under VZ's EFI firmware off the raw disk.
    loader = {
      systemd-boot.enable = true;
      efi.canTouchEfiVariables = false;
      timeout = 0;
    };
    # Serial console (hvc0) is streamed to the host by `boot-linux-gui` for boot
    # debugging; tty0 also writes to the virtio-gpu framebuffer.
    kernelParams = [
      "console=tty0"
      "console=hvc0"
    ];
    initrd.availableKernelModules = [
      "virtio_pci"
      "virtio_blk"
      "virtio_scsi"
      "usbhid"
      "sd_mod"
    ];
    # virtio-gpu DRM node so the compositor can run on a real (paravirtual)
    # display whose scanout VZ exposes to the host framebuffer.
    kernelModules = [ "virtio_gpu" ];
  };

  # make-disk-image (partitionTableType = "efi") labels the partitions ESP/nixos.
  fileSystems = {
    "/" = {
      device = "/dev/disk/by-label/nixos";
      fsType = "ext4";
    };
    "/boot" = {
      device = "/dev/disk/by-label/ESP";
      fsType = "vfat";
    };
  };

  # Software-rendering compositor stack.
  hardware.graphics.enable = true;
  environment.sessionVariables = {
    WLR_RENDERER = "pixman";
    WLR_RENDERER_ALLOW_SOFTWARE = "1";
  };

  # Autologin on tty1 and exec sway straight into the overlay.
  services.getty.autologinUser = "ix";
  users.users.ix = {
    isNormalUser = true;
    password = "";
    extraGroups = [
      "wheel"
      "video"
      "input"
    ];
  };
  programs.bash.loginShellInit = ''
    if [ "$(tty)" = /dev/tty1 ]; then
      exec ${lib.getExe pkgs.sway} -c ${swayConfig}
    fi
  '';

  environment.systemPackages = [
    pkgs.sway
    pkgs.grim
    pkgs.mesa
    pkgs.vulkan-loader
    pkgs.vulkan-tools
    bossbar-overlay
  ];

  # Trim the image: no docs, no network stack we do not need.
  documentation.enable = lib.mkDefault false;
  networking.useDHCP = lib.mkDefault false;
  system.stateVersion = "24.11";
}
