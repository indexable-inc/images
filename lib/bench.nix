# Consumer-facing helpers for declaring bench suites against the `indexbench`
# CLI.
#
# `mkBenchSuite` turns a small data description of a suite into the two outputs
# the framework distinguishes:
#
#   - `app`: a `nix run`-able wrapper that runs the suite's macro commands
#     through `indexbench run`, recording timing + RSS + custom metrics to the
#     history store and gating on regressions. Timing and RSS are not
#     reproducible inside the Nix sandbox, so this is a perf job (`apps.bench`),
#     never a flake check.
#   - `check`: an optional `nix flake check` derivation that runs a
#     consumer-provided allocation-count bench and gates it deterministically.
#     Allocation counts are reproducible, so they belong in `checks` where CI can
#     fail the build on any worsening.
#
# Keeping both paths behind one helper means a consumer declares a suite once and
# gets the reproducible gate and the perf job from the same description, rather
# than wiring the CLI by hand in two places.
{
  lib,
  writeNushellApplication,
}:
pkgs:
{
  # Suite name; becomes the `suite` field on every recorded run.
  name,
  # The built `indexbench` package (from `packages.<system>.indexbench`).
  indexbench,
  # Macro benches: a list of `{ name, command }`, where `command` is the shell
  # string run N times by the perf job. Each may print `@bench` lines to report
  # custom metrics.
  macros ? [ ],
  # Optional deterministic allocation check. When set to `{ bench = <exePath>; }`
  # (e.g. `lib.getExe someBenchBinary`), `check` runs that executable — which
  # must install `indexbench`'s counting allocator and print an
  # `@bench name=allocations ...` line — and gates it with the local store. Left
  # null, no `check` is produced.
  allocCheck ? null,
  # Runs per macro command in the perf job.
  runs ? 5,
}:
let
  exe = lib.getExe indexbench;

  cmdFlags = lib.concatMapStringsSep " " (
    entry: "--cmd ${lib.escapeShellArg entry.command} --cmd-name ${lib.escapeShellArg entry.name}"
  ) macros;

  app = writeNushellApplication pkgs {
    name = "bench-${name}";
    meta.description = "Run the ${name} bench suite (timing + RSS + custom metrics) through indexbench and gate on regressions";
    runtimeInputs = [
      indexbench
      pkgs.git
    ];
    # The wrapper forwards extra args (e.g. `--store local`, `--baseline <sha>`)
    # so a perf job can override the store or pin a baseline without a second
    # entry point.
    text = ''
      def --wrapped main [...args] {
        exec ${exe} run --suite ${lib.escapeShellArg name} --runs ${toString runs} ${cmdFlags} ...$args
      }
    '';
  };

  check =
    if allocCheck == null then
      null
    else
      pkgs.runCommand "bench-${name}-alloc-check"
        {
          nativeBuildInputs = [ indexbench ];
          # The consumer's bench executable must be reproducible (deterministic
          # alloc count); its store path is referenced here so the closure pins
          # the exact binary the gate runs.
          inherit (allocCheck) bench;
        }
        ''
          # Record into a sandbox-local store, then run a second time so the
          # comparator has a baseline. `--gate deterministic` makes only an
          # allocation-count regression fail the build; the bench's timing and
          # RSS metrics are non-reproducible in the sandbox and are ignored by
          # the gate, so this check stays a pure-eval-style reproducible gate.
          export HOME="$TMPDIR"
          store="$TMPDIR/store"

          # First pass establishes the baseline; allocation counts are
          # reproducible, so the two passes see identical counts and the gate
          # passes. A future regression changes the recorded count and trips it.
          ${exe} run --suite ${lib.escapeShellArg name} --store local --local-dir "$store" \
            --gate deterministic --cmd "$bench" --cmd-name alloc
          ${exe} run --suite ${lib.escapeShellArg name} --store local --local-dir "$store" \
            --gate deterministic --cmd "$bench" --cmd-name alloc

          mkdir -p "$out"
        '';
in
{
  inherit app;
}
// lib.optionalAttrs (check != null) { inherit check; }
