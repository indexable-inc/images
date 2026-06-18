{
  pkgs,
}:
let
  # nix-eval-jobs reports `cacheStatus` per attribute; nix-fast-build's
  # --skip-cached keys off it. For a floating content-addressed derivation
  # (the rust workspace units default to `contentAddressed = true`, see
  # lib/rust/cargo-unit.nix) the output path is not statically known at eval
  # time, so the stock queryCacheStatus skips the (null) output and only walks
  # the input drvs through queryMissing. That walk never consults the top-level
  # CA output's realisation, so an output already present in cache.ix.dev is
  # still reported `local`/`notBuilt` and nix-fast-build rebuilds every CA unit
  # on every CI run (~1434 of them), even on a fully warm cache.
  #
  # The patch resolves each unknown CA output's realisation against the
  # configured substituters (DrvOutput{staticOutputHash, outputName}) before
  # falling back to queryMissing, exactly as nix::Store::queryMissing itself
  # does for the floating-CA case (libstore/misc.cc). A resolved-and-cached
  # output then reports `cached` (with a real `outputs.<name>`), so --skip-cached
  # skips it. When an output has no realisation anywhere the helper bails to the
  # original queryMissing path, preserving the per-input neededBuilds breakdown
  # for genuinely-unbuilt drvs.
  #
  # Upstream: https://github.com/NixOS/nix/issues/12128
  #           https://github.com/nix-community/nix-eval-jobs/issues/403
  package = pkgs.nix-eval-jobs.overrideAttrs (old: {
    patches = (old.patches or [ ]) ++ [ ./ca-output-cache-status.patch ];
  });

  # The override's real risk is the C++ rebuild against nix's libstore linking
  # and the new symbols (staticOutputHashes, getDefaultSubstituters,
  # Store::queryRealisation) resolving at all, so the smoke test runs the
  # binary. `--help` exits 0 and prints usage without touching a store or
  # daemon (absent in the sandbox).
  smoke =
    pkgs.runCommand "nix-eval-jobs-smoke"
      {
        nativeBuildInputs = [ package ];
        strictDeps = true;
      }
      ''
        help=$(nix-eval-jobs --help 2>&1) || true
        case "$help" in
          *"--check-cache-status"*) ;;
          *)
            echo "nix-eval-jobs --help did not print usage" >&2
            printf '%s\n' "$help" >&2
            exit 1
            ;;
        esac
        mkdir -p "$out"
      '';
in
package.overrideAttrs (old: {
  passthru = (old.passthru or { }) // {
    tests = (old.passthru.tests or { }) // {
      inherit smoke;
    };
  };
  meta = (old.meta or { }) // {
    description = "nix-eval-jobs patched to report cacheStatus for floating content-addressed outputs (nix#12128 / nix-eval-jobs#403)";
    mainProgram = "nix-eval-jobs";
  };
})
