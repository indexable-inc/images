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
#   autoStart : bool        start the container (and thus map its window) at
#                           boot; default true. Debug scaffolding sets false
#                           and is started on demand from the guest console.
{pkgs}: let
  inherit (pkgs) lib;

  # portablemc's default wrapper bundles four full OpenJDKs (25/21/17/8,
  # ~1.9 GiB); its package.nix exposes `jvms` exactly to cut that closure.
  # MC 26.2 requires Java SE 25 minimum, so ship exactly jdk25 (the full JDK,
  # not headless: the client needs the AWT/X11 libs headless builds drop).
  portablemc = pkgs.portablemc.override {
    jvms = [pkgs.jdk25];
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

  # One binding for every place the version appears (positional arg, manifest
  # path, --fetch-exclude): a bump that misses one of them would silently let
  # portablemc re-fetch the manifest over the injection below. The paired
  # manual edit on a bump is options.txt's `version:` data-version (files
  # below).
  mcVersion = "26.2";

  # Everything after `start` that both portablemc invocations below share.
  mcStartArgs = builtins.concatStringsSep " " [
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
    mcVersion
  ];

  # 26.2 selects its startup renderer from the `--graphicsBackend vulkan`
  # GAME argument (joptsimple in net.minecraft.client.main.Main; the official
  # launcher passes it) — the options.txt `preferredGraphicsBackend` seed
  # below alone does not flip the startup backend (validated live: with only
  # the seed the game boots GL). portablemc 5.0.3 has no game-arg
  # passthrough, so inject the argument into the fetched version manifest's
  # `arguments.game`: the first run prepares every download with `--dry`,
  # jq appends the pair idempotently, and the real launch passes
  # `--fetch-exclude 26.2` so portablemc does not re-fetch the manifest over
  # the injection.
  mcLaunch = pkgs.writeBashApplication {
    name = "panes-mc-launch";
    runtimeInputs = [
      pkgs.jq
      # `mv` below: pinned rather than inherited from the service PATH, like
      # everything else in this file.
      pkgs.coreutils
    ];
    text = ''
      json=/var/lib/minecraft/versions/${mcVersion}/${mcVersion}.json
      # Re-fetch when the manifest is missing OR unparseable: a launch killed
      # mid-write leaves a truncated file that an existence-only guard would
      # accept, wedging the container in a jq-fail restart loop that never
      # re-downloads.
      if ! jq -e . "$json" >/dev/null 2>&1; then
        rm -f "$json"
        ${portablemc}/bin/portablemc --main-dir /var/lib/minecraft start --dry ${mcStartArgs}
        if [ ! -e "$json" ]; then
          echo "portablemc --dry did not produce $json" >&2
          exit 1
        fi
      fi
      jq \
        'if (.arguments.game | index("--graphicsBackend")) == null
         then .arguments.game += ["--graphicsBackend", "vulkan"]
         else . end' "$json" > "$json.tmp"
      mv "$json.tmp" "$json"
      exec ${portablemc}/bin/portablemc --main-dir /var/lib/minecraft start --fetch-exclude ${mcVersion} ${mcStartArgs}
    '';
    meta.description = "Minecraft ${mcVersion} launcher forcing the Vulkan startup backend (index#1686)";
  };
in {
  # Software (wl_shm) client: proves compositor + container + socket plumbing
  # with zero GPU involvement. weston-flower exists because nixpkgs builds
  # weston with -Ddemo-clients=true (weston-simple-shm does not: the pin sets
  # -Dsimple-clients= empty). Debug scaffolding, not a product surface: the
  # container ships but does not autostart (the bring-up era popped these
  # windows onto the desktop on every boot); start on the guest console with
  # `systemctl start container@demo` when debugging the shm path.
  demo = {
    command = "${pkgs.weston}/bin/weston-flower";
    env = {};
    binds = [];
    gpu = false;
    autoStart = false;
  };

  # A real interactive app: foot renders CPU-side into shm buffers, so it also
  # works with no GPU while exercising keyboard focus and resize. Same debug
  # scaffolding policy as `demo`: `systemctl start container@term` for an
  # interactive terminal window into the guest.
  term = {
    command = "${pkgs.foot}/bin/foot";
    env = {};
    binds = [];
    gpu = false;
    autoStart = false;
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
    # All downloads stay out of the ephemeral container root: this portablemc
    # build has no separate --work-dir; everything follows --main-dir (the
    # /var/lib/minecraft bind below). The wrapper exists solely to inject the
    # --graphicsBackend game argument (see mcLaunch above).
    command = lib.getExe mcLaunch;
    env = {
      # Pin the venus ICD so the loader cannot pick lavapipe, and prefer the
      # virtio-gpu PCI device (1af4:1050) if more than one ICD is visible.
      VK_DRIVER_FILES = "${pkgs.mesa}/share/vulkan/icd.d/virtio_icd.aarch64.json";
      MESA_VK_DEVICE_SELECT = "1af4:1050!";
      # 26.2's Vulkan backend hard-requires VK_KHR_synchronization2. The
      # guest mesa (indexable-inc/mesa fork, see ../guest-image/default.nix
      # and index#1742) now exposes sync2 natively by handling temporary sync
      # fd semaphore imports driver-side, so Khronos' emulation layer is a
      # passthrough no-op here: with force_enable unset it self-retires when
      # the driver advertises the extension. It stays wired as the fallback
      # if the driver ever masks sync2 again (e.g. a mesa bump without the
      # rebased fork patch). The manifest's library_path is store-absolute,
      # so VK_LAYER_PATH alone is enough for the loader inside the container.
      VK_LAYER_PATH = "${pkgs.vulkan-extension-layer}/share/vulkan/explicit_layer.d";
      VK_INSTANCE_LAYERS = "VK_LAYER_KHRONOS_synchronization2";
      XDG_SESSION_TYPE = "wayland";
      # nixpkgs openal (the portablemc wrapper's LD_LIBRARY_PATH provides it;
      # Maven's arm64 libopenal.so SIGSEGVs, so ./lwjgl-natives.nix omits it
      # and LWJGL falls through to this one) is built with the native
      # pipewire backend linked in (ALSOFT_DLOPEN=false, so libpipewire
      # resolves through the store rpath, no loader help needed). Pin that
      # backend: the guest's ONLY audio path is the PipeWire graph
      # (panes-sink -> vsock 7102 -> host CoreAudio, see ./nixos.nix), and
      # alsoft's other compiled-in backends (pulse, alsa) are dead ends
      # here -- its pulse probe once aborted the whole client via flite
      # (see textToSpeechSupport above). The container reaches the socket
      # through the /run/pipewire bind + PIPEWIRE_RUNTIME_DIR from clientEnv.
      ALSOFT_DRIVERS = "pipewire";
      # If Vulkan crashes at startup, 26.2 flips itself to "Prefer OpenGL"
      # (rewriting options.txt); that GL path should land on software
      # rendering. Diagnostic-only, data-only toggles (venus is the intended
      # renderer, do NOT set these by default):
      #   LIBGL_ALWAYS_SOFTWARE = "1";
      #   GALLIUM_DRIVER = "llvmpipe";
    };
    binds = ["/var/lib/minecraft"];
    gpu = true;
    files = {
      # Pre-seed the instance options. Seeded by a tmpfiles `C` rule, so
      # FIRST BOOT ONLY: an existing data disk keeps whatever the user
      # changed in-game. Every key/value below matches the serialization MC
      # 26.2 itself writes -- taken verbatim from the game-REWRITTEN
      # options.txt on the live data disk (the game rewrites the file on any
      # settings change, so its output is the authoritative grammar,
      # including `version:4903`, 26.2's data version; without that line the
      # game discards the file and falls back to the "Default" backend).
      #
      # Beyond the Vulkan backend, these are latency-opinionated defaults
      # (index#1686): MC's frames reach glass through the compositor's
      # ack-paced stream plus the host's display link, so anything that adds
      # in-game frame time or a second pacing loop stacks straight onto
      # mouse-look latency.
      # - enableVsync:false -- the dominant term, validated live by A/B:
      #   MC's vsync waits on the swapchain, stacking a second frame of
      #   pacing on top of the compositor's ack genlock (double vsync).
      # - maxFps:260 -- the slider's "Unlimited" stop (valid range 10-260).
      #   Uncapped, the frame the compositor ships is always the freshest
      #   sample; the ack pacing is what actually bounds the shipped rate.
      # - rawMouseInput:true -- GLFW raw relative motion (the default,
      #   pinned against stale instance state): pairs with
      #   zwp_relative_pointer fed by the host's uncoalesced deltas.
      # - graphicsPreset:"fancy" -- 26.2 replaced graphics *modes* with
      #   presets (the rewritten file has no `graphicsMode` key at all);
      #   fancy guards against Fabulous!, whose post-processing pipeline is
      #   pure added frame time, i.e. latency, on venus.
      # - narrator:0 -- off; belt and braces with the flite-less portablemc
      #   above (the narrator's TTS stack aborts the client without audio).
      "/var/lib/minecraft/options.txt" = pkgs.writeText "panes-mc-options.txt" ''
        version:4903
        preferredGraphicsBackend:"vulkan"
        enableVsync:false
        maxFps:260
        rawMouseInput:true
        graphicsPreset:"fancy"
        narrator:0
      '';
    };
  };
}
