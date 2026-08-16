{
  ix,
  pkgs ? ix.pkgs,
}:
# The conformance crate ships nothing: the PyO3 cdylib comes from the shared
# cargo-unit workspace graph, and the only artifacts worth building are the
# proofs that the generated Python surface behaves and stays describable.
# `run` installs the cdylib as `_conformance.abi3.so`, points the pinned
# interpreter at it, and runs `runner.py` (cancellation, backpressure,
# resource cleanup, zero-copy, GIL release, panic containment). `stubs`
# renders the host files with `unibind-gen py` from the same cdylib -- the
# surface exports objects, a resource, and streams, so it proves the stub
# emitter covers the whole phase-2 shape -- then checks that every intra-doc
# link came out in Python's spelling with no rustdoc syntax left, and
# strict-type-checks the result. Both join the CI check set through
# `passthru.tests` as `checks.<system>.unibind-conformance-{run,stubs}`.
let
  library = ix.rustWorkspace.units.libraries.unibind_conformance;

  # Locate the built extension: the unit output may suffix the metadata
  # hash, and the extension differs per OS. Same loop unibind's py.nix uses.
  findCdylib = ''
    cdylib=""
    for candidate in \
      ${library}/lib/libunibind_conformance.so \
      ${library}/lib/libunibind_conformance-*.so \
      ${library}/lib/libunibind_conformance.dylib \
      ${library}/lib/libunibind_conformance-*.dylib
    do
      if [ -f "$candidate" ]; then
        cdylib="$candidate"
        break
      fi
    done
    if [ -z "$cdylib" ]; then
      echo "unibind-conformance: no cdylib under ${library}/lib" >&2
      ls -la ${library}/lib >&2 || true
      exit 1
    fi
  '';

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
      ${findCdylib}
      install -m555 "$cdylib" "$site/_conformance.abi3.so"
      PYTHONPATH="$site" ${pkgs.python3.interpreter} ${./runner.py} 2>&1 | tee conformance.log
      touch $out
    '';

  stubs =
    pkgs.runCommand "unibind-conformance-stubs"
    {
      strictDeps = true;
      nativeBuildInputs = [
        ix.rustWorkspace.units.binaries.unibind-gen
        pkgs.mypy
      ];
      meta.description = "unibind-gen py host files for the conformance surface, strictly type-checked";
    }
    ''
      set -euo pipefail
      mkdir -p "$out"
      ${findCdylib}
      unibind-gen py --artifact "$cdylib" --package conformance --out "$out"

      # Each intra-doc link in the fixture, as Python spells its target. The
      # resolver already fails the build on a link that names nothing, so
      # what these add is the quieter half: a link that still resolves but
      # renders differently -- a rename, a `rename_all`, a variant that
      # stopped being SCREAMING_SNAKE -- changes one of these lines.
      stub="$out/conformance/_conformance.pyi"
      for rendered in \
        '`echo_record` hands one back unchanged' \
        'Horizontal coordinate, paired with `Point.y`.' \
        '`StrEnum` values, one to a `Finding`.' \
        '`escalate` promotes it to `Severity.HARD_FAILURE`.' \
        'Raises `Deliberate` on an empty label' \
        'refusal `Gate.named_after` makes.' \
        'Its bytes come back through `Shell.output`.' \
        'opened under; see `Gate`.' \
        'minted only by `Gate.open_shell`'
      do
        if ! grep -qF "$rendered" "$stub"; then
          echo "unibind-conformance-stubs: the generated stub is missing the rendered intra-doc link: $rendered" >&2
          exit 1
        fi
      done

      # The load-bearing one, because it holds for links nobody has written
      # yet: every link is rewritten into Python's spelling before emission,
      # so rustdoc's own syntax reaching a published stub means one went
      # unresolved -- which is exactly how a stale link would ship. A bare
      # `[` is not the marker (`dict[str, object]` is everywhere); the code
      # span opener is.
      if leftover=$(grep -rn -e '[[]`' "$out"); then
        echo "unibind-conformance-stubs: unresolved rustdoc intra-doc link syntax in the generated host files:" >&2
        echo "$leftover" >&2
        exit 1
      fi

      MYPYPATH="$out" MYPY_CACHE_DIR="$TMPDIR/mypy-cache" \
        mypy --strict --no-color-output -p conformance
    '';
in
  run.overrideAttrs (old: {
    passthru =
      (old.passthru or {})
      // {
        tests =
          (old.passthru.tests or {})
          // {
            inherit run stubs;
          };
      };
  })
