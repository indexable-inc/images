# indexbench

`packages/indexbench` is a metric-centric continuous-benchmarking framework for
the index repo. The core abstraction is a `Metric` (a named number with a unit
and a direction), not a time: wall-clock ns, peak RSS bytes, allocation counts,
match rates, and custom values all flow through one schema, one history store,
and one comparator (`src/lib.rs:1-26`). Three harnesses produce metrics, runs are
appended to durable history, and the comparator classifies each metric against a
baseline and exits non-zero on a regression so it can gate CI.

The statistical regression model (the three regimes, the two gates, the
Mann-Whitney test) is its own page: [gate.md](gate.md).

- Crate: `indexbench`, with a library (`src/lib.rs`), the `indexbench` CLI
  (`src/main.rs`), and the `indexbench-alloc-demo` example binary
  (`src/bin/alloc-demo.rs`) (`Cargo.toml:11-24`).
- Flake outputs: `nix run .#indexbench` (the bare CLI) and `nix run .#bench`
  (the repo's self-demo perf job, an `apps.bench` entry,
  `lib/per-system.nix:1108-1114`). Built by `ix.cargoUnit.selectBinaryWithTests`,
  `mainProgram = "indexbench"` (`default.nix:3-5`).

## Schema (`src/schema.rs`)

- `Metric { name, value, unit, lower_is_better, samples: Option<Vec<f64>> }`.
  `samples` present means a distributional metric (the comparator runs a
  significance test); absent means deterministic (exact compare). `value` is the
  headline number; for a distribution it is the median of `samples`, computed at
  construction (`src/schema.rs:18-86`). The presence of `samples`, not the metric
  name, picks the regime, so a new kind of measurement never touches the schema.
- `Run { suite, bench, metrics, machine_id, git_commit, git_dirty,
  timestamp_unix }` is one bench execution, the unit the store appends and the
  comparator diffs (`src/schema.rs:88-119`). `(machine_id, git_commit,
  timestamp_unix)` is the natural key.
- `machine_id()` is a 16-hex-char SHA-256 of hostname plus the first
  `/proc/cpuinfo` `model name` (hostname alone off Linux). It is stable per box
  but differs across CPUs, so timing baselines never cross hardware
  (`src/schema.rs:121-162`).

Runs serialize to JSON/JSONL; deterministic metrics omit `samples` entirely
(`#[serde(skip_serializing_if)]`). That JSON form is the contract.

## Three metric sources

- **Micro harness** (`src/micro.rs`) times a Rust closure in-process. `time_fn`
  runs a warm-up then a batched sampling loop and returns a distributional
  `wall_clock` (ns), one sample per measured round; the default `TimingConfig` is
  `warmup_iters = 100, samples = 30, batch = 100` (`src/micro.rs:102-165`).
  `bench_fn` adds a deterministic `allocations` count when the `CountingAllocator`
  global shim is installed. `count_allocations` probes the shim with a single
  `Box` and returns `None` (not a misleading `0`) when it is absent
  (`src/micro.rs:80-100`, `:167-189`). This is a small sampler, not a second
  copy of tango; tango stays the tool for paired A/B comparison of two builds in
  one process.
- **Macro harness** (`src/macro_harness.rs`) runs an external command N times.
  It reads `wall_clock` (ns) in the parent and `max_rss` (bytes) from
  `libc::wait4`'s per-child `rusage`. `wait4` is used directly rather than
  `std::process` (which discards `rusage`) and rather than
  `getrusage(RUSAGE_CHILDREN)` (which accumulates peak across all reaped
  children); `ru_maxrss` is normalized to bytes per platform (KiB on Linux, bytes
  on macOS) (`src/macro_harness.rs:117-249`). stdout and stderr are drained
  concurrently before reaping to avoid a full-pipe deadlock
  (`src/macro_harness.rs:151-168`). A non-zero exit or signal is a typed error
  (`src/macro_harness.rs:251-279`).
- **Custom metrics** are the extensibility hook. A benchmarked command prints
  lines of the form `@bench name=<id> value=<f64> unit=<str>
  lower_is_better=<bool>` to stdout or stderr; `unit` defaults to `count` and
  `lower_is_better` to `true` (`src/macro_harness.rs:26-107`). A malformed
  `@bench` line is a hard error, not a silent drop. The harness folds these into
  the same `Run`, so a consumer reports match-rate, force-steps, or NAR bytes
  without the framework knowing those metrics exist.

Aggregation across N runs (`aggregate`/`fold_custom`,
`src/macro_harness.rs:333-395`): `wall_clock` and `max_rss` become distributions
(one sample per run). A custom metric reported in every run also becomes a
distribution; one reported in only some runs stays deterministic (its last
value), since a partial sample set would mislead the test. First-run metadata
(unit, direction) wins.

## History store (`src/store.rs`)

`HistoryStore` is a small trait: `append` (durable before returning), `runs_for`
(ascending by timestamp), and the derived `previous_run` and `run_at_commit`
baseline lookups (`src/store.rs:31-88`). Two implementations:

- **`GitBranchStore` (default)** commits one `history.jsonl` blob to an orphan
  `bench-history` branch (`DEFAULT_BRANCH`) using pure git plumbing:
  `hash-object -w` writes the blob, `mktree` builds the one-entry tree,
  `commit-tree` makes the commit, and `update-ref` does a compare-and-swap
  against the tip read at start, so a concurrent append fails loudly instead of
  silently dropping a run (`src/store.rs:181-339`). The working tree and index
  are never touched, so it works mid-bench from a dirty checkout; the orphan
  branch shares no ancestry with `main`, so the growing JSONL never enters
  `main`'s tree. A CI job can `git push origin bench-history` to share results.
- **`LocalDirStore`** appends to `<dir>/history.jsonl` and `fsync`s, for laptop
  iteration and tests where committing every run is noise (`src/store.rs:116-179`).

A future object-store backend implements the same trait without touching the
harnesses or the comparator.

## CLI (`src/main.rs`)

Subcommands: `run`, `assert`, `history`, `viewer` (`src/main.rs:55-66`). `main`
maps any library error to `ExitCode::FAILURE` and prints it; the library never
panics on operational failure (`src/main.rs:175-184`).

- `run [--suite S] [--cmd CMD]... [--cmd-name N]... [--runs N] [--baseline SHA]
  [--threshold F] [--alpha F] [--gate all|deterministic] [--output-json]`
  executes a suite, records each run, and compares it to its baseline. The
  baseline is read before the run is appended, so a run is never its own baseline;
  the append happens before any reporting, so a later failure cannot lose the
  measurement (`src/main.rs:198-268`). `--runs` defaults to `DEFAULT_MACRO_RUNS`
  (10). Without `--cmd`, suite `self-demo` runs a built-in micro `fib` plus a
  macro `true` (`src/main.rs:362-401`). Exits non-zero per `--gate` (see
  [gate.md](gate.md)).
- `assert --cmd CMD --max METRIC=VALUE... [--runs N]` runs a command and gates
  each measured metric against a fixed budget, with no history. This is the
  hermetic, reproducible `nix flake check` path: a self-comparing run could only
  compare a binary against itself, so a fixed budget is what actually gates.
  `--runs` defaults to 1 so a deterministic metric stays deterministic; a metric
  over budget or never reported fails (`src/main.rs:140-155`, `:320-358`).
- `history --suite S --bench B` lists recorded runs for a `(suite, bench)`
  (`src/main.rs:404-427`).
- `viewer` is a documented stub for a planned HTML time-series viewer
  (`src/main.rs:437-443`).

Store selection is shared global flags (`StoreArgs`, `src/main.rs:68-100`):
`--store git|local` (default `git`), `--repo` (default `.`), `--branch` (default
`bench-history`), `--local-dir` (default `.indexbench`).

The runner (`src/run.rs`) ties it together: it resolves the git context
(`commit`, `dirty`; `unknown`/clean outside a repo), runs micro benches then
macro benches, stamps each `Run` with the machine id and timestamp, and returns
them for the CLI to record and compare (`src/run.rs:21-94`). A failing macro
bench aborts the whole invocation so a partial suite never records a misleading
baseline. Errors are one `snafu` enum (`src/error.rs`).

## Nix wiring (`lib/util/bench.nix`, `lib/per-system.nix`)

`ix.mkBenchSuite pkgs { name; indexbench; macros ? []; allocCheck ? null; runs ?
5 }` turns a suite description into two outputs (`lib/util/bench.nix:23-102`):

- `app`: a `writeNushellApplication` wrapper `bench-<name>` that runs the macro
  commands through `indexbench run --suite <name> --runs <runs> --cmd ...`,
  forwarding extra args (e.g. `--store local`, `--baseline`). This is a perf job
  (`apps.bench`), never a flake check, because timing and RSS are not
  reproducible in the sandbox.
- `check` (only when `allocCheck` is set): a `runCommand bench-<name>-alloc-check`
  that runs the consumer's bench once through `indexbench assert --runs 1 --max
  <metric>=<budget>`, the reproducible hermetic gate.

The repo's own suite is `indexbenchSelfDemo` (`lib/per-system.nix:404-420`):
`nix run .#bench` runs it as the perf job, and its `allocCheck` wires the
`indexbench-alloc-demo` binary into the `indexbench-self-demo-alloc` flake check
with `budgets.allocations = 64` (`lib/per-system.nix:958-963`). That binary
installs the `CountingAllocator` and makes exactly 64 `Box::new` allocations
(`EXPECTED_ALLOCATIONS`), so the count is a toolchain-stable constant and only an
actual added allocation reddens the gate (`src/bin/alloc-demo.rs:1-53`).

`nix develop .#bench` provides `indexbench` plus `hyperfine`, `valgrind`,
`samply`, and `jemalloc` (`lib/per-system.nix:1127-1135`). `tango-bench` is a
separate workspace dependency for paired A/B micro comparisons (see
`packages/file-search/benches`).

## Tests

`tests/loop.rs` is the end-to-end proof of the loop (run, record to a
`LocalDirStore`, a second run finds the first as baseline, report, and a
deterministic regression is gated), driving the library surface rather than the
CLI (`tests/loop.rs:1-13`). It is the passthru test target.
