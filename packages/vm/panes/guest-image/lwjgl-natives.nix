# Flat directory of LWJGL linux-arm64 JNI natives for Minecraft 26.2
# (index#1686). Mojang's version manifest ships no linux-arm64 natives at all,
# so the guest supplies LWJGL's own builds from Maven Central and the launch
# command points `-Dorg.lwjgl.librarypath` here (recipe validated in the live
# guest, 2026-07-01). MC 26.2 pins LWJGL 3.4.1; the module list and hashes
# live in ./pins.json (bump the version/urls there alongside the game
# version, then re-pin via the image's updateScript).
#
# Deliberately absent modules:
#   - lwjgl-openal: Maven's arm64 libopenal.so SIGSEGVs in this audio-less
#     guest. nixpkgs `openal` on the portablemc wrapper's LD_LIBRARY_PATH
#     works instead, so no libopenal.so may appear in this directory (LWJGL
#     prefers librarypath over the system path).
#   - lwjgl-vulkan: publishes no linux natives at all (the Vulkan binding
#     goes through libvulkan via GLFW, not a JNI native).
{
  lib,
  runCommand,
  fetchurl,
  unzip,
  # `loadPins ./pins.json` map, one entry per LWJGL module; injected by
  # ./default.nix (this file is data for the guest image, not a registry
  # package, so `ix` is not an autoArg here).
  pins,
}: let
  versions = lib.unique (lib.mapAttrsToList (_: pin: pin.version) pins);
  version = assert lib.assertMsg (
    builtins.length versions == 1
  ) "lwjgl-natives: pins.json entries must share one LWJGL version, got ${toString versions}";
    builtins.head versions;
  jars = lib.mapAttrsToList (_: pin: fetchurl {inherit (pin) url hash;}) pins;
in
  runCommand "lwjgl-natives-linux-arm64-${version}"
  {
    nativeBuildInputs = [unzip];
    inherit jars;
    strictDeps = true;
    __structuredAttrs = true;
    meta = {
      description = "LWJGL ${version} linux-arm64 natives extracted flat for -Dorg.lwjgl.librarypath";
      homepage = "https://www.lwjgl.org/";
      # The LWJGL natives themselves; the jars also bundle the upstream
      # libraries they bind (glfw, jemalloc, freetype, ...), each under its
      # own permissive license.
      license = lib.licenses.bsd3;
      sourceProvenance = [lib.sourceTypes.binaryNativeCode];
      platforms = ["aarch64-linux"];
    };
  }
  ''
    mkdir -p "$out" extract
    for jar in "''${jars[@]}"; do
      unzip -qo -d extract "$jar"
    done
    # The .so files sit under linux/arm64/org/lwjgl/** inside each jar
    # (META-INF carries same-named .so.git/.so.sha1 stamps, hence the filters).
    find extract -path '*/linux/arm64/*' -name '*.so' -not -path '*/META-INF/*' \
      -exec cp -t "$out" {} +
  ''
