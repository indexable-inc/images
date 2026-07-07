# The `swift` target of `unibind.lib.build`: generated Swift host files
# straight from the crate's built staticlib (the IR travels in its link
# sections), plus a compiled-and-run runner when the caller ships
# hand-written Swift sources (a conformance `main.swift`). Needs a Swift
# toolchain at build time, which nixpkgs only substitutes on darwin, so
# callers gate their package.nix `systems` to darwin.
{
  lib,
  pkgs,
  packageRegistry,
  rustWorkspace,
}: {
  crate,
  # Directory (and Swift module name) the generated files land under.
  package,
  # Directory with hand-written Swift sources compiled next to the generated
  # ones (a runner's `main.swift`). `null` means sources only, no runner.
  swiftSource ? null,
}: let
  entry =
    packageRegistry.byId.${crate}
      or (throw "unibind.lib.build: `${crate}` has no package.nix in the registry; add one with `inRustWorkspace = true`");

  libraryKey = lib.replaceStrings ["-"] ["_"] crate;
  library =
    rustWorkspace.units.libraries.${libraryKey}
      or (throw "unibind.lib.build: the shared workspace graph has no library unit `${libraryKey}` for `${crate}` (packages/${entry.relativePath}); the crate needs `crate-type = [\"staticlib\"]`");

  genBin = rustWorkspace.units.binaries."unibind-gen";

  # Locate the built staticlib: the unit output may suffix the metadata
  # hash. Same loop shape as the py target's cdylib lookup.
  findStaticlib = ''
    staticlib=""
    for candidate in \
      ${library}/lib/lib${libraryKey}.a \
      ${library}/lib/lib${libraryKey}-*.a
    do
      if [ -f "$candidate" ]; then
        staticlib="$candidate"
        break
      fi
    done
    if [ -z "$staticlib" ]; then
      echo "unibind: no staticlib under ${library}/lib" >&2
      ls -la ${library}/lib >&2 || true
      exit 1
    fi
  '';

  generated =
    pkgs.runCommand "unibind-${crate}-swift-generated"
    {
      strictDeps = true;
      nativeBuildInputs = [genBin];
      meta.description = "unibind-generated Swift host files for ${crate}";
    }
    ''
      set -euo pipefail
      ${findStaticlib}
      mkdir -p "$out"
      unibind-gen swift \
        --artifact "$staticlib" \
        --package ${lib.escapeShellArg package} \
        --out "$out"
    '';

  # runCommandCC: the swift-wrapper setup hook resolves the target C
  # compiler through `NIX_CC`, which the no-CC runCommand stdenv never sets.
  runner =
    pkgs.runCommandCC "unibind-${crate}-swift-runner"
    {
      strictDeps = true;
      nativeBuildInputs = [pkgs.swift];
      meta.description = "Compiled Swift runner for ${crate}";
    }
    ''
      set -euo pipefail
      ${findStaticlib}
      mkdir -p "$out/bin"
      swiftc -O \
        -import-objc-header ${generated}/${package}/include/bridging-header.h \
        ${generated}/${package}/*.swift \
        ${swiftSource}/*.swift \
        "$staticlib" \
        -o "$out/bin/${crate}-runner"
    '';

  conformance =
    pkgs.runCommand "unibind-${crate}-swift-conformance"
    {
      strictDeps = true;
      meta.description = "unibind swift conformance run for ${crate}";
    }
    ''
      set -euo pipefail
      mkdir -p "$out"
      ${runner}/bin/${crate}-runner | tee "$out/output.txt"
    '';
in
  {
    inherit generated library;
  }
  // lib.optionalAttrs (swiftSource != null) {
    inherit runner;
    tests = {inherit conformance;};
  }
