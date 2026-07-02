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
{ pkgs }:
let
  # portablemc's default wrapper bundles four full OpenJDKs (25/21/17/8,
  # ~1.9 GiB); its package.nix exposes `jvms` exactly to cut that closure.
  # One JRE that runs 1.16.5 is enough, passed explicitly via --jvm below.
  portablemc = pkgs.portablemc.override { jvms = [ pkgs.temurin-jre-bin-21 ]; };
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
  };

  # A real interactive app: foot renders CPU-side into shm buffers, so it also
  # works with no GPU while exercising keyboard focus and resize.
  term = {
    command = "${pkgs.foot}/bin/foot";
    env = { };
    binds = [ ];
  };

  # Minecraft 1.16.5: GL 2.1 era, matching the venus/zink GL ceiling (zink on
  # venus advertises GL 2.1, see index#1686). portablemc downloads the version
  # jar + assets on first launch (network via gvproxy) into /var/lib/minecraft,
  # which persists on the image's writable root fs.
  minecraft = {
    command = builtins.concatStringsSep " " [
      "${portablemc}/bin/portablemc"
      # Keep every download out of the ephemeral container root.
      "--main-dir /var/lib/minecraft"
      "--work-dir /var/lib/minecraft"
      "start"
      # temurin (not jdk21_headless): the client needs the non-headless JRE
      # libs (libawt_xawt and friends) that headless builds drop.
      "--jvm ${pkgs.temurin-jre-bin-21}/bin/java"
      # Offline session: no Microsoft account in the guest.
      "-u Panes"
      "1.16.5"
    ];
    env = {
      # GL 2.1 over the host GPU's Vulkan: mesa loads zink, zink sits on venus.
      MESA_LOADER_DRIVER_OVERRIDE = "zink";
      GALLIUM_DRIVER = "zink";
      # Pin the venus ICD so the loader cannot pick lavapipe, and prefer the
      # virtio-gpu PCI device (1af4:1050) if more than one ICD is visible.
      VK_DRIVER_FILES = "${pkgs.mesa}/share/vulkan/icd.d/virtio_icd.aarch64.json";
      MESA_VK_DEVICE_SELECT = "1af4:1050!";
      XDG_SESSION_TYPE = "wayland";
      # No LD_LIBRARY_PATH needed: the portablemc wrapper already prefixes it
      # with /run/opengl-driver/lib plus prismlauncher's runtime libs,
      # including glfw3-minecraft (a patched GLFW 3.4 with Wayland).
      # TODO(index#1686): MC 1.16's LWJGL extracts its own bundled GLFW 3.2
      # without Wayland support, so first launches may abort looking for X11.
      # Options to iterate on live: point LWJGL at the wrapper's
      # glfw3-minecraft (-Dorg.lwjgl.glfw.libname=libglfw.so with
      # GLFW_PLATFORM=wayland), or run Xwayland in the container. As a
      # last-resort diagnostic the whole GL stack can be forced to software by
      # adding LIBGL_ALWAYS_SOFTWARE = "1" here (data-only toggle, no module
      # change needed).
    };
    binds = [ "/var/lib/minecraft" ];
  };
}
