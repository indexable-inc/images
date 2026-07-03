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
  portablemc = pkgs.portablemc.override {
    jvms = [ pkgs.jdk25 ];
    # Keep flite OUT of the wrapper's LD_LIBRARY_PATH: MC's narrator speaks
    # through flite -> pulse, and with no audio stack in the guest
    # pa_simple_write aborts the whole client (validated live). Dropping the
    # lib beats a narrator=off option: nothing to load, nothing to abort.
    textToSpeechSupport = false;
  };

  # Mojang ships no linux-arm64 LWJGL natives; ./lwjgl-natives.nix supplies
  # LWJGL's own Maven builds, injected via the overlay in ./default.nix,
  # where the ./pins.json hashes live.
  lwjglNatives = pkgs.lwjgl-natives-linux-arm64;
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
  # into /var/lib/minecraft: the optional second `--disk` when attached (the
  # guest mounts /dev/vdb there, surviving image swaps; see ../README.md),
  # else the image's writable root fs.
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
      # LWJGL loads its JNI natives from here instead of the (absent)
      # manifest-provided linux-arm64 ones.
      "--jvm-arg=-Dorg.lwjgl.librarypath=${lwjglNatives}"
      # Make blaze3d pick the Wayland GLFW platform: left alone it requests
      # X11 even under Wayland ("GLFW error 0x1000E X11: DISPLAY missing").
      # MC 26.2's built-in debug switches (SharedConstants reads
      # MC_DEBUG_*-prefixed system properties, all gated on MC_DEBUG_ENABLED;
      # decompiled from 26.2 GLX/SharedConstants) flip the preference with
      # the stock Maven libglfw.so above, which is wayland-capable and
      # carries the preedit/IME API blaze3d binds. Validated live end-to-end:
      # the window maps on the host titled "Minecraft 26.2". If Mojang ever
      # drops the debug flag, the fallback is a Wayland-only glfw (X11
      # compiled out; openSUSE home:DarkWav glfw-minecraft recipe on
      # clear-code/glfw im-support) via -Dorg.lwjgl.glfw.libname; this repo
      # carried a packaged version of that until the debug flags landed (see
      # index#1686 and this file's history).
      "--jvm-arg=-DMC_DEBUG_ENABLED=true"
      "--jvm-arg=-DMC_DEBUG_PREFER_WAYLAND=true"
      # Offline session: no Microsoft account in the guest.
      "-u Panes"
      "26.2"
    ];
    env = {
      # Pin the venus ICD so the loader cannot pick lavapipe, and prefer the
      # virtio-gpu PCI device (1af4:1050) if more than one ICD is visible.
      VK_DRIVER_FILES = "${pkgs.mesa}/share/vulkan/icd.d/virtio_icd.aarch64.json";
      MESA_VK_DEVICE_SELECT = "1af4:1050!";
      # 26.2's Vulkan backend hard-requires VK_KHR_synchronization2, which
      # venus can never pass through on this host stack: mesa's venus driver
      # (vn_physical_device.c, vn_physical_device_get_passthrough_extensions)
      # gates sync2 on renderer-side SYNC_FD semaphore import, and the
      # macOS/MoltenVK renderer has no sync files — no virglrenderer/MoltenVK
      # pin bump changes that (MoltenVK itself has had sync2 since 1.2.6).
      # Khronos' emulation layer advertises the extension and translates
      # sync2 calls onto the core 1.2 venus device instead. The manifest's
      # library_path is store-absolute, so VK_LAYER_PATH alone is enough for
      # the loader inside the container.
      VK_LAYER_PATH = "${pkgs.vulkan-extension-layer}/share/vulkan/explicit_layer.d";
      VK_INSTANCE_LAYERS = "VK_LAYER_KHRONOS_synchronization2";
      XDG_SESSION_TYPE = "wayland";
      # nixpkgs openal (the portablemc wrapper's LD_LIBRARY_PATH provides it;
      # Maven's arm64 libopenal.so SIGSEGVs, so ./lwjgl-natives.nix omits it
      # and LWJGL falls through to this one) defaults to the pulse backend,
      # and the guest has no audio stack at all: force the null backend so
      # OpenAL initializes and MC runs silent instead of erroring (validated
      # live).
      ALSOFT_DRIVERS = "null";
      # If Vulkan crashes at startup, 26.2 flips itself to "Prefer OpenGL"
      # (rewriting options.txt); that GL path should land on software
      # rendering. Diagnostic-only, data-only toggles (venus is the intended
      # renderer, do NOT set these by default):
      #   LIBGL_ALWAYS_SOFTWARE = "1";
      #   GALLIUM_DRIVER = "llvmpipe";
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
