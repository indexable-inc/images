{
  ix,
  pkgs ? ix.pkgs,
}:
# The conformance crate ships nothing: the PyO3 cdylib comes from the shared
# cargo-unit workspace graph, and the only artifact worth building is the
# proof that the generated Python surface behaves. This derivation *is* that
# proof: install the cdylib as `_conformance.abi3.so`, point the pinned
# interpreter at it, and run `runner.py` (cancellation, backpressure,
# resource cleanup, zero-copy, GIL release, panic containment). It is also
# exposed as `passthru.tests.run` so it joins the CI check set as
# `checks.<system>.unibind-conformance-run`.
let
  run =
    pkgs.runCommand "unibind-conformance-run"
    {
      strictDeps = true;
      meta.description = "unibind phase-2 conformance runner over the generated Python bindings";
    }
    ''
      set -o pipefail
      site="$PWD/site"
      mkdir -p "$site"
      cdylib=""
      for candidate in \
        ${ix.rustWorkspace.units.libraries.unibind_conformance}/lib/libunibind_conformance.so \
        ${ix.rustWorkspace.units.libraries.unibind_conformance}/lib/libunibind_conformance-*.so \
        ${ix.rustWorkspace.units.libraries.unibind_conformance}/lib/libunibind_conformance.dylib \
        ${ix.rustWorkspace.units.libraries.unibind_conformance}/lib/libunibind_conformance-*.dylib
      do
        if [ -f "$candidate" ]; then
          cdylib="$candidate"
          break
        fi
      done
      if [ -z "$cdylib" ]; then
        echo "unibind-conformance: no cdylib under ${ix.rustWorkspace.units.libraries.unibind_conformance}/lib" >&2
        ls -la ${ix.rustWorkspace.units.libraries.unibind_conformance}/lib >&2 || true
        exit 1
      fi
      install -m555 "$cdylib" "$site/_conformance.abi3.so"
      PYTHONPATH="$site" ${pkgs.python3.interpreter} ${./runner.py} 2>&1 | tee conformance.log
      touch $out
    '';
in
  run.overrideAttrs (old: {
    passthru =
      (old.passthru or {})
      // {
        tests =
          (old.passthru.tests or {})
          // {
            inherit run;
          };
      };
  })
