# The `wasm` target of `unibind.lib.build`: the wasm-bindgen cdylib
# cross-compiled for wasm32-unknown-unknown through a target-scoped unit
# graph over the same workspace source and lock the native graph uses,
# `unibind-gen wasm` host files generated from its embedded IR, and the
# browser package directory (ESM wrapper + index.d.ts + schemas.ts + the
# wasm-bindgen `--target web` output under `wasm/`).
#
# `wasm-bindgen-cli` is the flake-pinned overlay package, and its version
# must equal the workspace's `wasm-bindgen` crate: the generated-schema
# contract between the two is not stable across patch releases, and a
# mismatch hard-fails at bind time.
{
  lib,
  pkgs,
  rustWorkspace,
  cargoUnit,
  rustToolchain,
}: {
  crate,
  # Directory holding the static package.json copied verbatim: a wasm
  # artifact is portable, so there is no `cpu`/`libc` stamping to do.
  npmSource,
}: let
  libraryKey = lib.replaceStrings ["-"] ["_"] crate;
  target = "wasm32-unknown-unknown";

  # Same shape as ix2nix-wasm (packages/ix2nix/wasm): a second
  # `buildWorkspace` is exactly the unit-identity change a different
  # target warrants. A rust-overlay toolchain carrying the target's
  # `rust-std`, pure-build policy (the native graph already runs
  # clippy/audit over these crates), input-addressed drvs, and
  # `embedMetadata = true` because this graph pins a stable toolchain
  # and `-Zembed-metadata=no` is nightly-only (ENG-12992).
  workspace = cargoUnit.buildWorkspace {
    pname = "${crate}-wasm32";
    inherit (rustWorkspace) src;
    cargoLock.lockFile = rustWorkspace.cargoLock;
    workspaceRoot = rustWorkspace.root;
    cargoArgs = ["-p" crate];
    inherit target;
    rustToolchain = rustToolchain {
      channel = "stable";
      version = "latest";
      targets = [target];
    };
    # wasm32-unknown-unknown ships no unwinder; `wasm-plugin` is release
    # plus panic=abort (the root manifest defines it for exactly this).
    profile = "wasm-plugin";
    policy = cargoUnit.policyPresets.pureBuild // {compiler.embedMetadata = true;};
    contentAddressed = false;
  };

  library =
    workspace.libraries.${libraryKey}
      or (throw "unibind.lib.build: the wasm32 graph has no library unit `${libraryKey}` for `${crate}`; the crate needs a cdylib target and a package.nix with inRustWorkspace");

  genBin = rustWorkspace.units.binaries.unibind-gen;

  findWasm = ''
    wasmfile=""
    for candidate in \
      ${library}/lib/${libraryKey}.wasm \
      ${library}/lib/${libraryKey}-*.wasm \
      ${library}/lib/*.wasm
    do
      if [ -f "$candidate" ]; then
        wasmfile="$candidate"
        break
      fi
    done
    if [ -z "$wasmfile" ]; then
      echo "unibind: no .wasm under ${library}/lib" >&2
      ls -la ${library}/lib >&2 || true
      exit 1
    fi
  '';

  # The browser package directory: the generated ESM index.js imports
  # `./wasm/<key>.js`, and wasm-bindgen writes that module (plus
  # `<key>_bg.wasm` beside it), so the import specifier and the emitted
  # file agree by construction.
  browser =
    pkgs.runCommand "unibind-${crate}-browser"
    {
      strictDeps = true;
      nativeBuildInputs = [
        genBin
        pkgs.coreutils
        pkgs.wasm-bindgen-cli
      ];
      passthru = {
        inherit library;
        # This wasm32 graph is its own `buildWorkspace`, invisible to the
        # shared workspace harvest; publish its IFD roots so consumers
        # substitute instead of re-vendoring the graph (#4127, same class
        # as ix2nix-wasm).
        workspaceIfdRoots = {
          inherit (workspace) unitsNix unitGraphJson vendorDir;
        };
      };
      meta.description = "unibind-generated browser package for ${crate} (ESM wrapper + index.d.ts + schemas.ts + wasm-bindgen web output)";
    }
    ''
      set -euo pipefail
      ${findWasm}

      mkdir -p "$out/wasm"
      unibind-gen wasm \
        --artifact "$wasmfile" \
        --module ./wasm/${libraryKey}.js \
        --out "$out"
      wasm-bindgen --target web \
        --out-dir "$out/wasm" \
        --out-name ${lib.escapeShellArg libraryKey} \
        "$wasmfile"

      cp ${npmSource}/package.json "$out/package.json"
    '';
in {
  inherit library browser;
}
