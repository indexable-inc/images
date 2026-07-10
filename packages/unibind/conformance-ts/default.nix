{
  ix,
  pkgs ? ix.pkgs,
}:
# The ts-backend conformance package through `ix.unibind.build`: the npm
# package (native addon + generated index.js/index.d.ts) is the package,
# with the Node end-to-end suite attached as the `node-conformance`
# passthru check. Gate for issue #1993's conformance matrix.
let
  built = ix.unibind.build {
    crate = "unibind-conformance-ts";
    targets.ts = {
      npmSource = builtins.path {
        name = "unibind-conformance-ts-npm-source";
        path = ./npm;
      };
    };
  };

  testSource = builtins.path {
    name = "unibind-conformance-ts-node-tests";
    path = ./tests/node;
  };

  # Records, error decoding, cancel-mid-flight dropping the Rust future,
  # stream backpressure and early close, `await using` disposal, and
  # GC-driven drop-without-close (what `--expose-gc` is for;
  # `--test-isolation=none` keeps that flag applied to the test code).
  # nodejs_24 has stable explicit resource management (`await using`).
  nodeConformance =
    pkgs.runCommand "unibind-conformance-ts-node"
    {
      strictDeps = true;
      nativeBuildInputs = [pkgs.nodejs_24];
      meta.description = "Node end-to-end suite over the unibind ts conformance package";
    }
    ''
      set -euo pipefail
      export UNIBIND_CONFORMANCE_PKG=${built.ts.npm}
      node --expose-gc --test --test-isolation=none \
        ${testSource}/conformance.test.mjs | tee "$out"
    '';
in
  built.ts.npm.overrideAttrs (old: {
    passthru =
      (old.passthru or {})
      // {
        inherit (built.ts) library;
        tests =
          (old.passthru.tests or {})
          // {
            node-conformance = nodeConformance;
          };
      };
  })
