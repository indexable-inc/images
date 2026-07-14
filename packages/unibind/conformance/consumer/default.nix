# The Rust-ABI runner of the unibind conformance suite: the sibling of the
# Python runner one directory up (`unibind-conformance-run`), proving the
# same boundary behaviors through the phase 4 stabby ABI instead of pyo3.
# It drives its own small engine crate (`../engine`) rather than the shared
# `_conformance` cdylib because that surface exports objects and `blocking`,
# which the Rust backend does not render yet; the engine mirrors the
# suite's cases (record/error round trip, async round trip, the
# cancellation drop-guard witness, a pull-based counting stream).
#
# The package is the consumer binary; its passthru.tests carry the two
# runCommand gates so they ride the registry's check wiring:
#
#  * `integration` runs the consumer against the built engine cdylib
#    (`checks.<system>.unibind-conformance-rs-integration`).
#  * `client-drift` regenerates the client crate through unibind-gen's
#    `rust` target (from the engine cdylib's embedded IR, the same one-
#    generator path `unibind.lib.build` exposes) and requires byte identity
#    with the committed crate.
#
# The consumer has no cargo dependency edge on the engine, and cargo-unit's
# per-crate filesets keep the engine's sources out of the client/consumer
# units, so an engine change re-runs these checks without recompiling either.
{
  ix,
  pkgs,
  ...
}: let
  engineClient = ix.unibind.build {
    crate = "unibind-conformance-engine";
    targets.rust = {
      crateName = "unibind-conformance-client";
    };
  };
  engineLibrary = engineClient.rust.library;
  consumerBinary = ix.rustWorkspace.units.binaries.unibind-conformance-consumer;
  # `.so` / `.dylib` per platform; cargo-unit may suffix the file name with a
  # metadata hash, hence the glob fallback in the script.
  sharedLibrary = pkgs.stdenv.hostPlatform.extensions.sharedLibrary;
  committedClient = builtins.path {
    name = "unibind-conformance-client-committed";
    path = ix.paths.packagesRoot + "/unibind/conformance/client";
  };
  integration =
    pkgs.runCommand "unibind-conformance-integration"
    {
      strictDeps = true;
      meta.description = "Conformance consumer run against the engine cdylib";
    }
    ''
      engine=""
      for candidate in \
        ${engineLibrary}/lib/libunibind_conformance_engine${sharedLibrary} \
        ${engineLibrary}/lib/libunibind_conformance_engine-*${sharedLibrary}; do
        if [ -e "$candidate" ]; then
          engine="$candidate"
          break
        fi
      done
      if [ -z "$engine" ]; then
        echo "no engine cdylib under ${engineLibrary}/lib" >&2
        ls -la ${engineLibrary}/lib >&2
        exit 1
      fi
      UNIBIND_CONFORMANCE_ENGINE="$engine" \
        ${consumerBinary}/bin/unibind-conformance-consumer
      mkdir -p "$out"
    '';
  clientDrift =
    pkgs.runCommand "unibind-conformance-client-drift"
    {
      strictDeps = true;
      nativeBuildInputs = [pkgs.diffutils];
      meta.description = "Committed conformance client is byte-identical to unibind-gen rs output";
    }
    ''
      diff -r ${engineClient.rust.generated} ${committedClient}
      mkdir -p "$out"
    '';
in
  ix.cargoUnit.selectBinaryWithTests ix.rustWorkspace.units {
    binary = "unibind-conformance-consumer";
    meta.description = "Run the generated unibind conformance client against the engine cdylib";
    passthru = {
      # Regeneration handle: `nix build .#unibind-conformance-consumer.generatedClient`
      # then copy the tree over packages/unibind/conformance/client.
      generatedClient = engineClient.rust.generated;
      tests = {
        inherit integration;
        client-drift = clientDrift;
      };
    };
  }
