# Declarative app catalog for the panes guest: one entry = one systemd-nspawn
# container = one Wayland client = (eventually) one macOS window. Data only,
# per the repo's data-before-commands rule; the machinery that renders these
# into NixOS `containers.<name>` lives in ./nixos.nix. See index#1686.
#
# Shape per app:
#   command : string        ExecStart line for the container's app service
#                           (absolute store path first, then arguments)
#   env     : attrset       extra environment merged over the Wayland defaults
#                           (WAYLAND_DISPLAY/XDG_RUNTIME_DIR); app values win
#   binds   : list of str   host paths bind-mounted read-write at the same path
#                           inside the container (persistent state, created by
#                           tmpfiles on the host)
#   gpu     : bool          bind /dev/dri (venus render node) into the
#                           container. Only for apps that render on the GPU:
#                           an nspawn bind with a missing source fails the
#                           whole container, so shm apps must not ask for it
#                           (the guest may boot GPU-less, see index#1686).
#   files   : attrset       host path -> store source, seeded once by host
#                           tmpfiles (`C`, first boot only) so an app starts
#                           with baked config it can still rewrite at runtime
{ pkgs }:
let
  # portablemc's default wrapper bundles four full OpenJDKs (25/21/17/8,
  # ~1.9 GiB); its package.nix exposes `jvms` exactly to cut that closure.
  # MC 26.2 requires Java SE 25 minimum, so ship exactly jdk25 (the full JDK,
  # not headless: the client needs the AWT/X11 libs headless builds drop).
  portablemc = pkgs.portablemc.override { jvms = [ pkgs.jdk25 ]; };
in
{
  # Software (wl_shm) client: proves compositor + container + socket plumbing
  # with zero GPU involvement. weston-flower exists because nixpkgs builds
  # weston with -Ddemo-clients=true (weston-simple-shm does not: the pin sets
  # -Dsimple-clients= empty).
  demo = {
    command = "${pkgs.weston}/bin/weston-flower";
    env = { };
    binds = [ ];
    gpu = false;
  };

  # A real interactive app: foot renders CPU-side into shm buffers, so it also
  # works with no GPU while exercising keyboard focus and resize.
  term = {
    command = "${pkgs.foot}/bin/foot";
    env = { };
    binds = [ ];
    gpu = false;
  };

  # Minecraft Java 26.2 ("Chaos Cubed"): ships a first-party experimental
  # Vulkan renderer, which rides venus directly, no zink and no mods
  # (requires Vulkan 1.2 + dynamic_rendering + push_descriptor; MoltenVK
  # provides both and venus passes host features through). portablemc
  # downloads the version jar + assets on first launch (network via gvproxy)
  # into /var/lib/minecraft, which persists on the image's writable root fs.
  minecraft = {
    command = builtins.concatStringsSep " " [
      "${portablemc}/bin/portablemc"
      # Keep every download out of the ephemeral container root. This
      # portablemc build has no separate --work-dir; everything follows
      # --main-dir.
      "--main-dir /var/lib/minecraft"
      "start"
      # 26.2 requires Java SE 25 minimum (see the portablemc jvms override).
      "--jvm ${pkgs.jdk25}/bin/java"
      # Offline session: no Microsoft account in the guest.
      "-u Panes"
      "26.2"
    ];
    env = {
      # Pin the venus ICD so the loader cannot pick lavapipe, and prefer the
      # virtio-gpu PCI device (1af4:1050) if more than one ICD is visible.
      VK_DRIVER_FILES = "${pkgs.mesa}/share/vulkan/icd.d/virtio_icd.aarch64.json";
      MESA_VK_DEVICE_SELECT = "1af4:1050!";
      XDG_SESSION_TYPE = "wayland";
      # No LD_LIBRARY_PATH needed: the portablemc wrapper already prefixes it
      # with /run/opengl-driver/lib plus prismlauncher's runtime libs,
      # including glfw3-minecraft (GLFW 3.4 with Wayland) and vulkan-loader.
      # If Vulkan crashes at startup, 26.2 flips itself to "Prefer OpenGL"
      # (rewriting options.txt); that GL path should land on software
      # rendering, forceable as a diagnostic by adding
      # LIBGL_ALWAYS_SOFTWARE = "1" here (data-only toggle, no module change).
    };
    binds = [ "/var/lib/minecraft" ];
    gpu = true;
    files = {
      # Pre-seed the instance options so first launch renders through Vulkan
      # ("Prefer Vulkan" in Video Settings; key/values per minecraft.wiki
      # Options.txt, added in 26.2-snapshot-1). The `version` line is 26.2's
      # data version and is mandatory: the game discards an options.txt that
      # lacks it and would fall back to the "Default" backend (OpenGL in the
      # 26.2 release).
      "/var/lib/minecraft/options.txt" = pkgs.writeText "panes-mc-options.txt" ''
        version:4903
        preferredGraphicsBackend:"vulkan"
      '';
    };
  };
}
