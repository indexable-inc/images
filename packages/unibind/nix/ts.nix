# The `ts` target of `unibind.lib.build`: the napi addon cdylib from the
# shared workspace graph, `unibind-gen ts` host files generated from its
# embedded IR, and the tui-node-shaped npm package directory (native addon
# + index.js + index.d.ts + stamped package.json).
{
  lib,
  pkgs,
  rustWorkspace,
}: {
  crate,
  # Directory holding the static package.json to stamp with `cpu`/`libc`
  # (the crate's `npm/`).
  npmSource,
}: let
  libraryKey = lib.replaceStrings ["-"] ["_"] crate;
  library =
    rustWorkspace.units.libraries.${libraryKey}
      or (throw "unibind.lib.build: the shared workspace graph has no library unit `${libraryKey}` for `${crate}`; the crate needs a cdylib target and a package.nix with inRustWorkspace");

  genBin = rustWorkspace.units.binaries."unibind-gen";

  # npm's `cpu`/`libc` fields gate installation. The addon is host-built, so
  # stamp the arch of this build; a nixpkgs build is always glibc. The npm
  # packaging (patchelf + glibc stamping) is Linux-only, same policy as the
  # py target's wheel.
  npmCpu =
    {
      x86_64-linux = "x64";
      aarch64-linux = "arm64";
    }
    .${
      pkgs.stdenv.hostPlatform.system
    }
      or (throw "unibind: npm packages are Linux-only, got ${pkgs.stdenv.hostPlatform.system}");

  # Locate the built addon cdylib: the unit output may suffix the metadata
  # hash. Only `.so`: the npm layout is Linux-only (see npmCpu).
  findCdylib = ''
    cdylib=""
    for candidate in \
      ${library}/lib/lib${libraryKey}.so \
      ${library}/lib/lib${libraryKey}-*.so
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

  # The npm package directory: the generated index.js loads
  # `./native/<addon>.node`, and the addon basename is the library key, so
  # the require path and the copied file agree by construction.
  npm =
    pkgs.runCommand "unibind-${crate}-npm"
    {
      strictDeps = true;
      nativeBuildInputs = [
        genBin
        pkgs.coreutils
        pkgs.jq
        pkgs.patchelf
        pkgs.removeReferencesTo
      ];
      passthru = {inherit library;};
      meta.description = "unibind-generated npm package for ${crate} (native addon + index.js + index.d.ts)";
    }
    ''
      set -euo pipefail
      ${findCdylib}

      mkdir -p "$out/native"
      cp "$cdylib" "$out/native/${libraryKey}.node"
      chmod u+w "$out/native/${libraryKey}.node"

      unibind-gen ts \
        --artifact "$out/native/${libraryKey}.node" \
        --addon ${lib.escapeShellArg libraryKey} \
        --out "$out"

      # Stamp the artifact's arch/libc so npm refuses to install it on a
      # host that cannot dlopen this single host-built addon.
      jq '. + { cpu: ["${npmCpu}"], libc: ["glibc"] }' \
        ${npmSource}/package.json >"$out/package.json"

      # Strip the build-time rpath and nixpkgs toolchain references so the
      # package is not pinned to this store path.
      if patchelf --print-rpath "$out/native/${libraryKey}.node" >/dev/null 2>&1; then
        patchelf --remove-rpath "$out/native/${libraryKey}.node"
      fi
      remove-references-to \
        -t ${pkgs.glibc} \
        -t ${pkgs.stdenv.cc.cc.lib} \
        "$out/native/${libraryKey}.node"
    '';
in {
  inherit library npm;
}
