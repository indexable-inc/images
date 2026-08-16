{
  ix,
  pkgs ? ix.pkgs,
}:
# The wasm-backend conformance package through `ix.unibind.build`: the
# browser package (wasm-bindgen web output + generated
# index.js/index.d.ts/schemas.ts) is the package, with the Node end-to-end
# suite attached as the `node-conformance` passthru check. Wasm row of
# issue #1993's conformance matrix.
let
  built = ix.unibind.build {
    crate = "unibind-conformance-wasm";
    targets.wasm = {
      npmSource = builtins.path {
        name = "unibind-conformance-wasm-npm-source";
        path = ./npm;
      };
    };
  };

  testSource = builtins.path {
    name = "unibind-conformance-wasm-node-tests";
    path = ./tests/node;
  };

  # Same invariants the ts suite pins, over the wasm boundary: records both
  # directions, error decoding off the reason channel, abort mid-flight
  # dropping the Rust future, stream pull/early close/null-at-end, and
  # GC-driven drop-without-close (what `--expose-gc` is for;
  # `--test-isolation=none` keeps that flag applied to the test code).
  # nodejs_24 runs the wasm-bindgen `--target web` output via
  # `init(readFileSync(...))`: no fetch, no DOM, so what passes here is the
  # module contract, not a browser emulation.
  nodeConformance =
    pkgs.runCommand "unibind-conformance-wasm-node"
    {
      strictDeps = true;
      nativeBuildInputs = [pkgs.nodejs_24];
      meta.description = "Node end-to-end suite over the unibind wasm conformance package";
    }
    ''
      set -euo pipefail
      export UNIBIND_CONFORMANCE_PKG=${built.wasm.browser}
      node --expose-gc --test --test-isolation=none \
        ${testSource}/conformance.test.mjs | tee "$out"
    '';
in
  built.wasm.browser.overrideAttrs (old: {
    passthru =
      (old.passthru or {})
      // {
        inherit (built.wasm) library;
        tests =
          (old.passthru.tests or {})
          // {
            node-conformance = nodeConformance;
          };
      };
  })
