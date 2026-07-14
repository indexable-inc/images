# The `rust` target of `unibind.lib.build`: the generated `<name>-client`
# crate rendered from the crate's built cdylib. The IR travels in the
# cdylib's link section, so the emitted client (and the IR hash its load
# handshake compares) is provably derived from the artifact that ships,
# never from re-parsing Rust source.
{
  lib,
  pkgs,
  rustWorkspace,
}: {
  crate,
  # The generated crate's `[package] name`.
  crateName ? "${crate}-client",
  # `workspace = true` dependencies plus the package.nix registry marker,
  # or concrete versions for use outside this workspace.
  workspaceDeps ? true,
}: let
  libraryKey = lib.replaceStrings ["-"] ["_"] crate;
  library =
    rustWorkspace.units.libraries.${libraryKey}
      or (throw "unibind.lib.build: the shared workspace graph has no library unit `${libraryKey}` for `${crate}`");

  genBin = rustWorkspace.units.binaries."unibind-gen";

  # Locate the built cdylib: the unit output may suffix the metadata hash,
  # and the extension differs per OS. Same loop as py.nix.
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

  generated =
    pkgs.runCommand "unibind-${crate}-rs-client"
    {
      strictDeps = true;
      nativeBuildInputs = [genBin];
      meta.description = "unibind-generated Rust client crate for ${crate}";
    }
    ''
      set -euo pipefail
      ${findCdylib}
      mkdir -p "$out"
      unibind-gen rs \
        --artifact "$cdylib" \
        --crate-name ${lib.escapeShellArg crateName} \
        --out "$out" ${lib.optionalString workspaceDeps "--workspace-deps"}
    '';
in {
  inherit generated library;
}
