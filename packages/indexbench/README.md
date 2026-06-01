# indexbench

A metric-centric continuous-benchmarking framework for the index repo.

The core abstraction is a **Metric**, not a time. A metric is any named number
with a unit and a direction (`lower_is_better`): wall-clock nanoseconds, peak RSS
bytes, allocation counts, a match rate, NAR bytes, force-resolve steps. Harnesses
produce metrics; the comparator and reporter consume them. Adding a new kind of
measurement never touches the schema.

## Schema

```rust
Metric { name: String, value: f64, unit: String, lower_is_better: bool, samples: Option<Vec<f64>> }
Run    { suite, bench, metrics: Vec<Metric>, machine_id, git_commit, git_dirty, timestamp_unix }
```

`samples` present ⇒ distributional metric (gets a statistical test); absent ⇒
deterministic metric (exact compare). `machine_id` is a stable hash of hostname +
CPU model, so timing baselines never cross hardware. Runs serialize to JSON/JSONL;
that form is the contract.

## Three metric sources

- **Micro harness** (`micro`): times a Rust closure with a warm-up + batched
  sampling loop (distributional `wall_clock`), and — behind the
  `CountingAllocator` `#[global_allocator]` shim — reports a deterministic
  `allocations` count.
- **Macro harness** (`macro_harness`): runs an external command N times, reading
  `wall_clock` in the parent and `max_rss` from `libc::wait4`'s **per-child**
  `rusage` (avoiding the `getrusage(RUSAGE_CHILDREN)` accumulation across reaped
  children). `ru_maxrss` is normalized to bytes per platform (KiB on Linux, bytes
  on macOS). No instrumentation needed to be timed and sized.
- **Custom metrics**: the extensibility hook. A benchmarked command prints lines
  of the form

  ```text
  @bench name=<id> value=<f64> unit=<str> lower_is_better=<bool>
  ```

  (`unit` defaults to `count`, `lower_is_better` to `true`). The harness folds
  them into the same `Run`, so a consumer reports RSS, match-rate, force-steps, or
  NAR bytes without the framework knowing those metrics exist.

## History store

Runs are appended to a durable store keyed by `(machine_id, git_commit,
timestamp_unix)`. The store is a trait with two implementations:

- **`GitBranchStore` (default):** commits JSONL to an orphan `bench-history`
  branch in the repo, using git plumbing so the working tree is never touched.
  Chosen as the default because it is durable, versioned, and shared with zero new
  infrastructure: anyone with the repo has the full history, a CI job can
  `git push origin bench-history`, and the data is auditable like any commit.
  Because the branch shares no ancestry with `main`, the growing JSONL never
  enters `main`'s tree.
- **`LocalDirStore`:** a `history.jsonl` in a directory, for laptop iteration and
  tests where committing every run is noise.

A future object-store backend implements the same trait without touching the
harnesses or the comparator.

## Comparator

Default baseline: the previous run on the same machine (override with
`--baseline <commit>`).

- **Distributional** metrics (`samples` present, `n >= 8` with spread on both
  sides): a two-sided Mann-Whitney U test (normal approximation with tie
  correction) for significance, plus a relative effect-size threshold
  (default 2%). A metric is a regression only when the change is **both**
  statistically significant **and** beyond the threshold. Mann-Whitney is used
  over Welch's t because bench timings are routinely right-skewed.
- **Thresholded** metrics (`samples` present but too few or zero spread, e.g. RSS
  that reports an identical peak each run): the effect-size threshold alone
  decides, so a sub-threshold environmental wobble does not trip the gate.
- **Deterministic** metrics (no `samples`, e.g. an allocation count): exact
  compare. Any worsening is a regression — there is no noise to absorb.

The CLI exits non-zero on any regression (the CI gate).

## CLI

```sh
# Run the built-in self-demo suite, record to history, compare to the previous run:
nix run index#bench               # the apps.bench perf job
nix run index#indexbench -- run   # the bare CLI

# Run an ad-hoc macro bench, 10 runs, gate on regression vs the previous run:
nix run index#indexbench -- run --suite mysuite --cmd "my-tool --work" --runs 10

# Compare against a fixed commit instead of the previous run:
nix run index#indexbench -- run --suite mysuite --cmd "my-tool" --baseline <sha>

# Gate only on deterministic (reproducible) metrics, comparing to history:
nix run index#indexbench -- run --cmd "my-tool" --gate deterministic

# Gate a metric against a fixed budget with NO history (the hermetic flake-check
# path): fails if the measured allocation count exceeds 64.
nix run index#indexbench -- assert --cmd "my-tool" --runs 1 --max allocations=64

# JSON output for CI / the (stubbed) viewer:
nix run index#indexbench -- run --output-json

# Inspect history:
nix run index#indexbench -- history --suite mysuite --bench my-tool
```

By default runs record to the `bench-history` git branch; pass `--store local
--local-dir <dir>` for a directory-backed store. `assert` needs no store: it
compares each measured metric against a fixed `--max` budget, which is what makes
a reproducible metric (an allocation count) usable as a hermetic `nix flake
check` — a self-comparing run could only ever compare a binary against itself.

## Declaring a suite (Nix)

```nix
# In lib/per-system.nix (or any consumer), declare a suite once and get both the
# perf job and the reproducible alloc-count gate:
mySuite = ix.mkBenchSuite pkgs {
  name = "search";
  indexbench = repoPackages.indexbench;
  macros = [
    { name = "index-1k"; command = "file-search index ./corpus"; }
  ];
  # Optional: gate a deterministic allocation count as a flake check. The bench
  # prints `@bench name=allocations ...`; the check fails if it exceeds budget.
  allocCheck = {
    bench = lib.getExe myAllocBenchBinary;
    budgets.allocations = 64;
  };
};
# mySuite.app   -> wire into apps.bench / a perf job  (timing + RSS; NOT a check)
# mySuite.check -> wire into checks                   (deterministic; reproducible)
```

## Reproducible vs non-reproducible

Deterministic alloc-count metrics are reproducible, so they belong in `nix flake
check` via `indexbench assert` against a fixed budget (CI fails when a count
exceeds it). Timing and RSS are sandbox-sensitive and belong in `apps.bench` (the
perf job), never in flake checks. The framework provides both paths from one
suite description.

## Profiling shell

```sh
nix develop index#bench   # indexbench + hyperfine + valgrind + samply + jemalloc
```

`tango-bench` is already a workspace dependency for paired A/B micro comparisons
(see `packages/file-search/benches`).

## Fast-follow

`indexbench viewer` is a stub. The planned feature renders the JSONL history as
an interactive HTML time-series per `(machine, suite, bench, metric)`.
