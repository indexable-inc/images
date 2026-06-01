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
#     consumer-provided allocation-count bench once and asserts each declared
#     metric stays within its budget (`indexbench assert`). Allocation counts are
#     reproducible, so this is a real hermetic gate: pushing a count above its
#     budget fails the build, while raising the budget is a deliberate one-line
#     change. (A self-comparing two-run gate cannot catch a regression — both
#     runs measure the same binary — so a fixed budget is what actually gates.)
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
  # Optional deterministic allocation check. Set to
  # `{ bench = <exePath>; budgets = { <metric> = <max>; ... }; }`:
  #   - `bench` (e.g. `lib.getExe someBenchBinary`) must install `indexbench`'s
  #     counting allocator and print `@bench name=allocations ...` lines.
  #   - `budgets` maps each metric to its upper bound; the check fails if a
  #     measured metric exceeds its budget or is never reported.
  # Left null, no `check` is produced.
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

  budgetFlags = lib.concatStringsSep " " (
    lib.mapAttrsToList (metric: max: "--max ${lib.escapeShellArg "${metric}=${toString max}"}") (
      allocCheck.budgets or { }
    )
  );

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
          # Run the bench once and assert each metric is within its budget.
          # `--runs 1` keeps the allocation count deterministic (no distribution
          # folding); the budget is a fixed number, so this is a real hermetic
          # gate — an added allocation exceeds the budget and fails the build,
          # while timing/RSS (non-reproducible in the sandbox) are simply not
          # budgeted here and live in the apps.bench perf job instead.
          ${exe} assert --cmd "$bench" --runs 1 ${budgetFlags}

          mkdir -p "$out"
        '';
in
{
  inherit app;
}
// lib.optionalAttrs (check != null) { inherit check; }
