# The `jvm` target of `unibind.lib.build`: generated Java Panama + Kotlin
# host sources straight from the crate's built cdylib (the IR travels in its
# link section). Compilation of the generated sources (a JDK 22+ toolchain)
# is the consumer's concern; this target only materializes them.
{
  lib,
  pkgs,
  rustWorkspace,
}: {crate}: let
  libraryKey = lib.replaceStrings ["-"] ["_"] crate;
  library =
    rustWorkspace.units.libraries.${libraryKey}
    or (throw "unibind.lib.build: the shared workspace graph has no library unit `${libraryKey}` for `${crate}`; the crate must build a cdylib with the unibind `jvm` feature enabled");

  genBin = rustWorkspace.units.binaries."unibind-gen";

  # Locate the built extension: the unit output may suffix the metadata
  # hash, and the extension differs per OS. Same loop as ./py.nix.
  findCdylib = ''
    cdylib=""
    for candidate in \
      ${library}/lib/lib${libraryKey}.so \
      ${library}/lib/lib${libraryKey}-*.so \
      ${library}/lib/lib${libraryKey}.dylib \
      ${library}/lib/lib${libraryKey}-*.dylib
    do
      if [ -f "$candidate" ]; then
        cdylib="$candidate"
        break
      fi
    done
    if [ -z "$cdylib" ]; then
      echo "unibind: no cdylib under ${library}/lib" >&2
      ls -la ${library}/lib >&2 || true
      exit 1
    fi
  '';

  sources =
    pkgs.runCommand "unibind-${crate}-jvm-sources"
    {
      strictDeps = true;
      nativeBuildInputs = [genBin];
      meta.description = "unibind-generated Java + Kotlin host sources for ${crate}";
    }
    ''
      set -euo pipefail
      ${findCdylib}
      mkdir -p "$out"
      unibind-gen jvm \
        --artifact "$cdylib" \
        --out "$out"
    '';
in {
  inherit library sources;
}
