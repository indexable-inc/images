# indexbench regression gate

The comparator (`src/compare.rs`) classifies each candidate metric against a
baseline metric of the same name and decides whether it regressed. This is the
mechanism behind the CLI's non-zero exit. See [overview.md](overview.md) for the
schema, harnesses, and CLI; this page is the statistical model.

## Baseline selection

The default baseline is the previous run on the same machine, supplied by the
store's `previous_run` (filtered to `machine_id`); `--baseline <commit>` pins a
specific commit's run via `run_at_commit` instead
(`src/main.rs:219-223`, `src/store.rs:53-87`). Metrics are matched by name. A
candidate metric with no baseline counterpart is reported as `NoBaseline` (never
a regression); a baseline-only metric is dropped, since a metric the bench
stopped emitting is not a behavior regression (`src/compare.rs:167-186`). A
bench's first-ever run uses `first_run`, which reports every metric as
`NoBaseline` with its measured value intact so `--output-json` is not empty
(`src/compare.rs:188-206`).

## The three regimes (`regime_for`, `src/compare.rs:269-298`)

The regime is chosen from the data, not the metric name:

- **Deterministic** - the candidate or baseline has no `samples` (an allocation
  count, a one-shot byte size). Exact compare: any worsening is a `Regression`,
  any bettering an `Improvement`, regardless of magnitude. There is no noise to
  absorb, which is exactly what makes deterministic metrics usable as flake
  checks (`exact_verdict`, `src/compare.rs:336-355`).
- **Distributional** - both sides have `samples`, each with at least
  `MIN_SAMPLES = 8` values and non-zero spread. A two-sided Mann-Whitney U test
  gives significance and a relative effect-size threshold gives materiality; a
  metric regresses only when the change is both significant and beyond the
  threshold (`src/compare.rs:231-241`).
- **Thresholded** - `samples` are present but a side has fewer than `MIN_SAMPLES`
  or zero spread (e.g. RSS that reports an identical peak every run). A rank test
  on identical values is meaningless and an exact compare would trip on a
  sub-threshold environmental wobble, so the effect-size threshold alone decides;
  no p-value is reported (`src/compare.rs:242-248`).

`has_spread` treats an all-equal sample set as having none
(`src/compare.rs:300-308`).

## Effect size and direction (`src/compare.rs:310-334`)

`relative_change = (candidate - baseline) / baseline`, then sign-flipped for a
higher-is-better metric so that negative always means improvement and one rule
reads both directions. It is `None` when the baseline value is zero. `classify`
turns an oriented change plus a "meaningful" flag into the verdict: not meaningful
is `Unchanged`; meaningful and positive is `Regression`; meaningful and negative
is `Improvement`. The default threshold is `DEFAULT_THRESHOLD = 0.02` (2%),
overridable with `--threshold` (`src/compare.rs:51-56`).

## The Mann-Whitney U test (`mann_whitney_u_pvalue`, `src/compare.rs:357-424`)

The distributional significance test is a two-sided Mann-Whitney U via the normal
approximation with a tie correction. Mann-Whitney is chosen over Welch's t
because bench timings are routinely right-skewed (GC pauses, scheduler hiccups)
and a rank test assumes no normality.

- Pool both sample sets, assign average ranks (ties get the block's mean rank),
  and sum group A's ranks.
- Compute `U`, its mean, and a tie-corrected variance; if the variance is
  non-positive (no spread once ties are removed), return `p = 1.0`.
- Apply a 0.5 continuity correction toward the mean, form the z-score, and convert
  to a two-sided p-value via the standard-normal CDF, itself built from an
  Abramowitz and Stegun `erf` approximation (`src/compare.rs:426-455`).

The approximation is accurate for the sample sizes a bench produces
(`MIN_SAMPLES` and up), so an exact U distribution is unnecessary. Significance is
`p < alpha`, default `DEFAULT_ALPHA = 0.05`, overridable with `--alpha`
(`src/compare.rs:51-52`, `:237`).

`DEFAULT_MACRO_RUNS = 10` is deliberately above `MIN_SAMPLES` so the built-in
`wall_clock`/`max_rss` distributions reach the distributional regime by default;
a compile-time assertion guards that floor (`src/compare.rs:35-49`).

## Verdicts and the two gates

`Verdict` is `Improvement`, `Regression`, `Unchanged`, or `NoBaseline`
(`src/compare.rs:58-73`). A `Comparison` exposes two gate predicates
(`src/compare.rs:128-147`):

- `has_regression()` is true if any metric regressed. The `--gate all` mode (the
  default, the perf job) uses it, so timing, RSS, and deterministic metrics all
  count.
- `has_deterministic_regression()` is true only if a `Deterministic`-regime
  metric regressed. The `--gate deterministic` mode uses it so a sandbox-noisy
  timing/RSS verdict cannot fail the build while a worsened allocation count
  still does.

The CLI ORs the selected predicate across every compared run and returns
`ExitCode::FAILURE` when any regressed (`src/main.rs:225-267`).

## Reporting (`src/report.rs`)

`human_table` renders one row per metric: candidate value, baseline, percent
delta (oriented so a minus sign is improvement), a significance marker, and the
verdict label. The marker is `=` for deterministic (exact), `~` for thresholded,
`***` for a significant distributional change (`p < alpha`), and blank for a
not-significant distributional metric (`src/report.rs:32-59`). `--output-json`
serializes the `Comparison` verbatim for CI or the planned viewer to consume.
