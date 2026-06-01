//! Compare a candidate run against a baseline and classify each metric.
//!
//! The default baseline is "the previous run on the same machine" (the
//! [store](crate::store) supplies it); a fixed commit can be pinned instead. For
//! each metric the classifier picks one of three regimes:
//!
//! - **Distributional** (`samples` present, `n >= MIN_SAMPLES` with spread on
//!   both sides): a two-sided Mann-Whitney U test (normal approximation with a
//!   tie correction) for significance, plus a relative effect-size threshold
//!   (default 2%). A metric is a regression only when the change is *both*
//!   statistically significant *and* beyond the threshold — that keeps the gate
//!   from firing on tiny-but-significant noise or on large-but-noisy swings.
//! - **Thresholded** (`samples` present but a side has too few or zero spread,
//!   e.g. RSS reporting identical peaks within a run): the effect-size threshold
//!   alone decides. A rank test on identical values is meaningless, but exact
//!   compare would flag a sub-threshold environmental wobble, so the threshold
//!   is the right middle ground for a sampled-but-stable metric.
//! - **Deterministic** (`samples` absent, e.g. an allocation count): exact
//!   compare. Any worsening is a regression; there is no noise to tolerate. This
//!   is what makes deterministic metrics usable as flake checks.
//!
//! Mann-Whitney is chosen over Welch's t because bench timings are routinely
//! right-skewed (GC pauses, scheduler hiccups), and a rank test does not assume
//! normality. See the test module for the worked significance/noise/regression
//! cases.

use serde::Serialize;

use crate::schema::{Metric, Run};

/// Minimum samples on each side for the distributional regime.
///
/// Below this the U-statistic's normal approximation is unreliable, so the
/// metric falls back to the thresholded regime instead.
pub const MIN_SAMPLES: usize = 8;

/// Default number of macro runs (one sample each).
///
/// It must clear [`MIN_SAMPLES`] so the built-in `wall_clock`/`max_rss`
/// distributions reach the distributional regime by default, instead of
/// silently falling back to thresholded as a sub-floor default would.
pub const DEFAULT_MACRO_RUNS: u32 = 10;

// Compile-time guard: a default below the floor would quietly disable the
// Mann-Whitney path for every default-configured macro bench.
const _: () = assert!(
    DEFAULT_MACRO_RUNS as usize >= MIN_SAMPLES,
    "DEFAULT_MACRO_RUNS must reach the distributional floor MIN_SAMPLES",
);

/// Default two-sided significance level for the Mann-Whitney test.
pub const DEFAULT_ALPHA: f64 = 0.05;

/// Default relative effect-size threshold: changes within ±2% are treated as
/// unchanged even when statistically significant.
pub const DEFAULT_THRESHOLD: f64 = 0.02;

/// How a single metric moved relative to its baseline.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Verdict {
    /// The metric got better beyond the threshold (and significantly, when
    /// distributional).
    Improvement,
    /// The metric got worse beyond the threshold (and significantly, when
    /// distributional). This is what fails the CI gate.
    Regression,
    /// No meaningful change.
    Unchanged,
    /// The baseline run did not carry this metric, so there is nothing to
    /// compare against. Reported, never a regression.
    NoBaseline,
}

/// Which statistical regime classified a metric.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Regime {
    /// Mann-Whitney U on the two sample sets (samples present, `n >= MIN_SAMPLES`
    /// with spread on both sides).
    Distributional,
    /// A sampled metric whose data cannot support a significance test (too few
    /// samples, or zero spread on a side), compared by the effect-size threshold
    /// alone. Used for noisy-but-stable metrics like RSS that report identical
    /// peaks within a run; an environmental wobble under the threshold stays
    /// unchanged rather than tripping an exact compare.
    Thresholded,
    /// A truly deterministic metric (`samples` absent, e.g. an allocation count):
    /// exact compare, any worsening is a regression.
    Deterministic,
}

/// The comparison of one metric against its baseline.
#[derive(Debug, Clone, Serialize)]
pub struct MetricComparison {
    /// The metric name.
    pub name: String,
    /// The metric unit.
    pub unit: String,
    /// Baseline headline value, or `None` when the baseline lacked the metric.
    pub baseline_value: Option<f64>,
    /// Candidate headline value.
    pub candidate_value: f64,
    /// Signed relative change `(candidate - baseline) / baseline`, oriented so
    /// negative is always an improvement regardless of `lower_is_better`. `None`
    /// when there is no baseline or the baseline value is zero.
    pub relative_change: Option<f64>,
    /// The classification regime used.
    pub regime: Regime,
    /// Two-sided p-value from Mann-Whitney, present only in the distributional
    /// regime.
    pub p_value: Option<f64>,
    /// The verdict.
    pub verdict: Verdict,
}

/// The full comparison of a candidate run against its baseline.
#[derive(Debug, Clone, Serialize)]
pub struct Comparison {
    /// The suite/bench identity (same on both sides).
    pub suite: String,
    /// The bench name.
    pub bench: String,
    /// Per-metric comparisons, in candidate order.
    pub metrics: Vec<MetricComparison>,
}

impl Comparison {
    /// Whether any metric regressed. The CLI's default gate exits non-zero when
    /// this is true — used by the perf job, where timing and RSS are in scope.
    #[must_use]
    pub fn has_regression(&self) -> bool {
        self.metrics.iter().any(|m| m.verdict == Verdict::Regression)
    }

    /// Whether any *deterministic* metric regressed. This is the reproducible
    /// gate: a flake check uses it so a sandbox-noisy timing/RSS verdict cannot
    /// fail the build, while a worsened allocation count still does.
    #[must_use]
    pub fn has_deterministic_regression(&self) -> bool {
        self.metrics
            .iter()
            .any(|m| m.verdict == Verdict::Regression && m.regime == Regime::Deterministic)
    }
}

/// Tunable thresholds for a comparison.
#[derive(Debug, Clone, Copy)]
pub struct CompareConfig {
    /// Two-sided significance level for the distributional regime.
    pub alpha: f64,
    /// Relative effect-size threshold (fractional, e.g. `0.02` for 2%).
    pub threshold: f64,
}

impl Default for CompareConfig {
    fn default() -> Self {
        Self {
            alpha: DEFAULT_ALPHA,
            threshold: DEFAULT_THRESHOLD,
        }
    }
}

/// Compare a `candidate` run against a `baseline` run.
///
/// Metrics are matched by name. A candidate metric with no baseline counterpart
/// is reported as [`Verdict::NoBaseline`]. Baseline-only metrics are dropped —
/// the candidate defines the current metric set, and a metric a bench stopped
/// emitting is not a regression of the bench's behavior.
#[must_use]
pub fn compare(baseline: &Run, candidate: &Run, config: CompareConfig) -> Comparison {
    let metrics = candidate
        .metrics
        .iter()
        .map(|metric| compare_metric(baseline.metric(&metric.name), metric, config))
        .collect();

    Comparison {
        suite: candidate.suite.clone(),
        bench: candidate.bench.clone(),
        metrics,
    }
}

/// Build a baseline-less comparison: every candidate metric reported as
/// [`Verdict::NoBaseline`] with its measured value intact.
///
/// Used for a bench's first-ever run, where there is nothing to diff against but
/// the measurements must still surface (so `--output-json` on a first run is not
/// an empty metric list). Reuses [`compare_metric`] with no baseline so the
/// `NoBaseline` shape has a single source of truth.
#[must_use]
pub fn first_run(run: &Run) -> Comparison {
    Comparison {
        suite: run.suite.clone(),
        bench: run.bench.clone(),
        metrics: run
            .metrics
            .iter()
            .map(|metric| compare_metric(None, metric, CompareConfig::default()))
            .collect(),
    }
}

/// Classify one candidate metric against its (optional) baseline counterpart.
fn compare_metric(baseline: Option<&Metric>, candidate: &Metric, config: CompareConfig) -> MetricComparison {
    let Some(baseline) = baseline else {
        return MetricComparison {
            name: candidate.name.clone(),
            unit: candidate.unit.clone(),
            baseline_value: None,
            candidate_value: candidate.value,
            relative_change: None,
            regime: regime_for(None, candidate),
            p_value: None,
            verdict: Verdict::NoBaseline,
        };
    };

    let relative_change = relative_change(baseline.value, candidate.value, candidate.lower_is_better);
    let regime = regime_for(Some(baseline), candidate);

    let (p_value, verdict) = match regime {
        Regime::Distributional => {
            // Both sides have samples in this regime (regime_for guarantees it).
            let base = baseline.samples.as_deref().unwrap_or_default();
            let cand = candidate.samples.as_deref().unwrap_or_default();
            let p = mann_whitney_u_pvalue(base, cand);
            let significant = p < config.alpha;
            let beyond = relative_change.is_some_and(|rc| rc.abs() > config.threshold);
            let verdict = classify(relative_change, significant && beyond);
            (Some(p), verdict)
        }
        Regime::Thresholded => {
            // Sampled but untestable: a change counts only if it clears the
            // effect-size threshold. No significance claim, so no p-value.
            let beyond = relative_change.is_some_and(|rc| rc.abs() > config.threshold);
            let verdict = classify(relative_change, beyond);
            (None, verdict)
        }
        Regime::Deterministic => {
            // Exact compare: any worsening is a regression, any bettering is an
            // improvement, regardless of magnitude. There is no noise to absorb.
            let verdict = exact_verdict(baseline.value, candidate.value, candidate.lower_is_better);
            (None, verdict)
        }
    };

    MetricComparison {
        name: candidate.name.clone(),
        unit: candidate.unit.clone(),
        baseline_value: Some(baseline.value),
        candidate_value: candidate.value,
        relative_change,
        regime,
        p_value,
        verdict,
    }
}

/// Choose the regime for a candidate metric and its optional baseline.
///
/// - No `samples` on the candidate (or no baseline) ⇒ [`Regime::Deterministic`]:
///   the metric is an exact value (alloc count, byte size), exact-compared.
/// - `samples` present but a side has fewer than [`MIN_SAMPLES`] or zero spread
///   ⇒ [`Regime::Thresholded`]: a rank test would be meaningless, so the
///   effect-size threshold alone decides. This keeps an intentionally sampled
///   metric (RSS, time) out of the exact-compare path even when one recording
///   happened to be perfectly stable.
/// - `samples` present with enough data and spread on both sides ⇒
///   [`Regime::Distributional`]: full Mann-Whitney plus threshold.
fn regime_for(baseline: Option<&Metric>, candidate: &Metric) -> Regime {
    let Some(baseline) = baseline else {
        return Regime::Deterministic;
    };
    if candidate.samples.is_none() || baseline.samples.is_none() {
        return Regime::Deterministic;
    }
    let testable = |metric: &Metric| {
        metric
            .samples
            .as_deref()
            .is_some_and(|s| s.len() >= MIN_SAMPLES && has_spread(s))
    };
    if testable(baseline) && testable(candidate) {
        Regime::Distributional
    } else {
        Regime::Thresholded
    }
}

/// Whether a sample set has any spread; an all-equal set has none.
fn has_spread(samples: &[f64]) -> bool {
    let Some(first) = samples.first() else {
        return false;
    };
    // Exact inequality is what "any spread" means; `total_cmp` expresses it
    // without tripping `clippy::float_cmp`.
    samples.iter().any(|s| !s.total_cmp(first).is_eq())
}

/// Signed relative change oriented so negative is always an improvement.
///
/// For a `lower_is_better` metric the raw change already points the right way;
/// for a higher-is-better metric we flip the sign so a verdict can read the same
/// rule for both. `None` when the baseline value is zero (relative change is
/// undefined).
fn relative_change(baseline: f64, candidate: f64, lower_is_better: bool) -> Option<f64> {
    if baseline == 0.0 {
        return None;
    }
    let raw = (candidate - baseline) / baseline;
    Some(if lower_is_better { raw } else { -raw })
}

/// Turn an oriented relative change plus a "meaningful" flag into a verdict.
fn classify(relative_change: Option<f64>, meaningful: bool) -> Verdict {
    if !meaningful {
        return Verdict::Unchanged;
    }
    match relative_change {
        Some(rc) if rc > 0.0 => Verdict::Regression,
        Some(rc) if rc < 0.0 => Verdict::Improvement,
        _ => Verdict::Unchanged,
    }
}

/// Exact verdict for a deterministic metric: any worsening regresses.
fn exact_verdict(baseline: f64, candidate: f64, lower_is_better: bool) -> Verdict {
    let worse = if lower_is_better {
        candidate > baseline
    } else {
        candidate < baseline
    };
    let better = if lower_is_better {
        candidate < baseline
    } else {
        candidate > baseline
    };
    if worse {
        Verdict::Regression
    } else if better {
        Verdict::Improvement
    } else {
        Verdict::Unchanged
    }
}

/// Two-sided p-value of the Mann-Whitney U test via the normal approximation
/// with a tie correction.
///
/// Returns `1.0` (no evidence of a difference) when either side is empty. The
/// approximation is accurate for the sample sizes a bench produces
/// (`MIN_SAMPLES` and up); an exact U distribution is unnecessary at these
/// sizes. Reference: Mann & Whitney (1947); tie correction per Hollander &
/// Wolfe, *Nonparametric Statistical Methods*.
#[must_use]
#[expect(
    clippy::cast_precision_loss,
    reason = "sample counts and ranks are tiny (bench sample sets), far below 2^52, so the f64 statistics are exact"
)]
pub fn mann_whitney_u_pvalue(a: &[f64], b: &[f64]) -> f64 {
    let n1 = a.len();
    let n2 = b.len();
    if n1 == 0 || n2 == 0 {
        return 1.0;
    }

    // Pool both samples, assign average ranks, and sum the ranks of group `a`.
    let mut pooled: Vec<(f64, u8)> = Vec::with_capacity(n1 + n2);
    pooled.extend(a.iter().map(|&v| (v, 0u8)));
    pooled.extend(b.iter().map(|&v| (v, 1u8)));
    pooled.sort_by(|x, y| x.0.total_cmp(&y.0));

    let mut rank_sum_a = 0.0;
    let mut tie_correction = 0.0;
    let mut index = 0usize;
    while index < pooled.len() {
        let mut end = index + 1;
        // Exact equality groups a tie block; `total_cmp` gives the same answer
        // as `==` here without tripping `clippy::float_cmp`.
        while end < pooled.len() && pooled[end].0.total_cmp(&pooled[index].0).is_eq() {
            end += 1;
        }
        let tie_len = end - index;
        // Average rank for this tie block (ranks are 1-based).
        let average_rank = (index + 1 + end) as f64 / 2.0;
        for entry in &pooled[index..end] {
            if entry.1 == 0 {
                rank_sum_a += average_rank;
            }
        }
        let t = tie_len as f64;
        // t^3 - t, written as a fused multiply-add for accuracy.
        tie_correction += (t * t).mul_add(t, -t);
        index = end;
    }

    let n1f = n1 as f64;
    let n2f = n2 as f64;
    let nf = n1f + n2f;

    let u1 = rank_sum_a - n1f * (n1f + 1.0) / 2.0;
    let mean_u = n1f * n2f / 2.0;

    let variance = (n1f * n2f / 12.0) * ((nf + 1.0) - tie_correction / (nf * (nf - 1.0)));
    if variance <= 0.0 {
        // No spread once ties are removed: the two samples are indistinguishable.
        return 1.0;
    }

    // Continuity correction of 0.5 toward the mean.
    let diff = (u1 - mean_u).abs();
    let z = (diff - 0.5).max(0.0) / variance.sqrt();
    two_sided_p_from_z(z)
}

/// Two-sided p-value for a standard-normal z, `2 * (1 - Phi(|z|))`, using a
/// rational approximation of the error function (Abramowitz & Stegun 7.1.26).
fn two_sided_p_from_z(z: f64) -> f64 {
    let p = 2.0 * (1.0 - standard_normal_cdf(z.abs()));
    p.clamp(0.0, 1.0)
}

/// Standard-normal CDF `Phi(x)` via `erf`. `0.5 * (1 + erf(x/√2))` is the
/// midpoint of `1` and `erf`, so `f64::midpoint` states it directly.
fn standard_normal_cdf(x: f64) -> f64 {
    f64::midpoint(1.0, erf(x / std::f64::consts::SQRT_2))
}

/// `erf` via Abramowitz & Stegun 7.1.26 (max error ~1.5e-7), enough for a
/// bench gate's p-values. The polynomial is evaluated in Horner form with fused
/// multiply-adds for accuracy.
fn erf(x: f64) -> f64 {
    let sign = if x < 0.0 { -1.0 } else { 1.0 };
    let x = x.abs();

    let t = 1.0 / 0.327_591_1f64.mul_add(x, 1.0);
    // Horner evaluation of the degree-5 A&S polynomial in `t`.
    let poly = 1.061_405_429f64
        .mul_add(t, -1.453_152_027)
        .mul_add(t, 1.421_413_741)
        .mul_add(t, -0.284_496_736)
        .mul_add(t, 0.254_829_592);
    let y = (poly * t).mul_add(-(-x * x).exp(), 1.0);
    sign * y
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run_with(metric: Metric) -> Run {
        Run {
            suite: "s".to_owned(),
            bench: "b".to_owned(),
            metrics: vec![metric],
            machine_id: "m".to_owned(),
            git_commit: "c".to_owned(),
            git_dirty: false,
            timestamp_unix: 0,
        }
    }

    #[test]
    fn deterministic_any_worsening_regresses() {
        let base = run_with(Metric::deterministic("allocations", 10.0, "count", true));
        let cand = run_with(Metric::deterministic("allocations", 11.0, "count", true));
        let result = compare(&base, &cand, CompareConfig::default());
        assert_eq!(result.metrics[0].regime, Regime::Deterministic);
        assert_eq!(result.metrics[0].verdict, Verdict::Regression);
        assert!(result.has_regression());
    }

    #[test]
    fn deterministic_improvement_is_flagged() {
        let base = run_with(Metric::deterministic("allocations", 10.0, "count", true));
        let cand = run_with(Metric::deterministic("allocations", 8.0, "count", true));
        let result = compare(&base, &cand, CompareConfig::default());
        assert_eq!(result.metrics[0].verdict, Verdict::Improvement);
        assert!(!result.has_regression());
    }

    #[test]
    fn deterministic_equal_is_unchanged() {
        let base = run_with(Metric::deterministic("allocations", 10.0, "count", true));
        let cand = run_with(Metric::deterministic("allocations", 10.0, "count", true));
        let result = compare(&base, &cand, CompareConfig::default());
        assert_eq!(result.metrics[0].verdict, Verdict::Unchanged);
    }

    #[test]
    fn distributional_clear_slowdown_is_a_regression() {
        // A baseline tightly around 100 vs a candidate tightly around 130: a
        // large, significant, beyond-threshold slowdown.
        let base = Metric::distribution("wall_clock", "ns", true, (0..12).map(|i| 100.0 + f64::from(i % 3)).collect());
        let cand = Metric::distribution("wall_clock", "ns", true, (0..12).map(|i| 130.0 + f64::from(i % 3)).collect());
        let result = compare(&run_with(base), &run_with(cand), CompareConfig::default());
        let m = &result.metrics[0];
        assert_eq!(m.regime, Regime::Distributional);
        assert_eq!(m.verdict, Verdict::Regression, "p={:?} rc={:?}", m.p_value, m.relative_change);
    }

    #[test]
    fn distributional_overlapping_noise_is_unchanged() {
        // Two samples drawn from the same spread: large overlap, no significant
        // difference, so the gate must not fire.
        let pattern = [100.0, 102.0, 98.0, 101.0, 99.0, 103.0, 97.0, 100.0, 101.0, 99.0, 102.0, 98.0];
        let base = Metric::distribution("wall_clock", "ns", true, pattern.to_vec());
        let mut shifted = pattern;
        shifted.reverse();
        let cand = Metric::distribution("wall_clock", "ns", true, shifted.to_vec());
        let result = compare(&run_with(base), &run_with(cand), CompareConfig::default());
        assert_eq!(result.metrics[0].verdict, Verdict::Unchanged);
        assert!(!result.has_regression());
    }

    #[test]
    fn distributional_tiny_but_significant_change_is_below_threshold() {
        // A perfectly separated but ~1% shift: Mann-Whitney sees significance,
        // yet the 2% effect-size threshold keeps it from being a regression.
        let base = Metric::distribution("wall_clock", "ns", true, (0..12).map(|i| f64::from(i).mul_add(0.01, 1000.0)).collect());
        let cand = Metric::distribution("wall_clock", "ns", true, (0..12).map(|i| f64::from(i).mul_add(0.01, 1009.0)).collect());
        let result = compare(&run_with(base), &run_with(cand), CompareConfig::default());
        let m = &result.metrics[0];
        assert!(m.p_value.is_some_and(|p| p < DEFAULT_ALPHA), "expected significance, p={:?}", m.p_value);
        assert!(m.relative_change.is_some_and(|rc| rc.abs() < DEFAULT_THRESHOLD), "expected sub-threshold, rc={:?}", m.relative_change);
        assert_eq!(m.verdict, Verdict::Unchanged);
    }

    #[test]
    fn higher_is_better_drop_is_a_regression() {
        // match_rate is higher-is-better; a drop must regress.
        let base = Metric::distribution("match_rate", "ratio", false, (0..12).map(|i| 0.90 + f64::from(i % 2) * 0.001).collect());
        let cand = Metric::distribution("match_rate", "ratio", false, (0..12).map(|i| 0.70 + f64::from(i % 2) * 0.001).collect());
        let result = compare(&run_with(base), &run_with(cand), CompareConfig::default());
        assert_eq!(result.metrics[0].verdict, Verdict::Regression);
    }

    #[test]
    fn deterministic_gate_ignores_distributional_regressions() {
        // A run with a worsened (thresholded/distributional) timing but an
        // unchanged alloc count: the perf gate fires, the deterministic gate
        // does not. This is exactly the flake-check vs perf-job split.
        let base = Run {
            suite: "s".to_owned(),
            bench: "b".to_owned(),
            metrics: vec![
                Metric::distribution("wall_clock", "ns", true, vec![100.0; 10]),
                Metric::deterministic("allocations", 10.0, "count", true),
            ],
            machine_id: "m".to_owned(),
            git_commit: "c".to_owned(),
            git_dirty: false,
            timestamp_unix: 0,
        };
        let cand = Run {
            metrics: vec![
                Metric::distribution("wall_clock", "ns", true, vec![200.0; 10]),
                Metric::deterministic("allocations", 10.0, "count", true),
            ],
            ..base.clone()
        };
        let result = compare(&base, &cand, CompareConfig::default());
        assert!(result.has_regression(), "the 2x timing slowdown is a perf regression");
        assert!(
            !result.has_deterministic_regression(),
            "the unchanged alloc count means no reproducible regression"
        );
    }

    #[test]
    fn deterministic_gate_fires_on_alloc_regression() {
        let base = Run {
            suite: "s".to_owned(),
            bench: "b".to_owned(),
            metrics: vec![Metric::deterministic("allocations", 10.0, "count", true)],
            machine_id: "m".to_owned(),
            git_commit: "c".to_owned(),
            git_dirty: false,
            timestamp_unix: 0,
        };
        let cand = Run {
            metrics: vec![Metric::deterministic("allocations", 11.0, "count", true)],
            ..base.clone()
        };
        let result = compare(&base, &cand, CompareConfig::default());
        assert!(result.has_deterministic_regression(), "a worsened alloc count is a reproducible regression");
    }

    #[test]
    fn first_run_reports_every_metric_as_no_baseline_with_values() {
        // A first run has no baseline, but its measurements must still appear
        // (the `--output-json` first-run path), each marked NoBaseline and never
        // a regression.
        let run = Run {
            suite: "s".to_owned(),
            bench: "b".to_owned(),
            metrics: vec![
                Metric::distribution("wall_clock", "ns", true, vec![100.0; 10]),
                Metric::deterministic("allocations", 42.0, "count", true),
            ],
            machine_id: "m".to_owned(),
            git_commit: "c".to_owned(),
            git_dirty: false,
            timestamp_unix: 0,
        };
        let comparison = first_run(&run);
        assert_eq!(comparison.metrics.len(), 2, "every measured metric surfaces");
        assert!(comparison.metrics.iter().all(|m| m.verdict == Verdict::NoBaseline));
        assert!(comparison.metrics.iter().all(|m| m.baseline_value.is_none()));
        let allocations = comparison.metrics.iter().find(|m| m.name == "allocations").expect("allocations present");
        assert!((allocations.candidate_value - 42.0).abs() < 1e-9, "the measured value is preserved");
        assert!(!comparison.has_regression(), "a first run never regresses");
    }

    #[test]
    fn missing_baseline_metric_is_no_baseline() {
        let base = run_with(Metric::deterministic("allocations", 10.0, "count", true));
        let cand = run_with(Metric::deterministic("brand_new", 5.0, "count", true));
        let result = compare(&base, &cand, CompareConfig::default());
        assert_eq!(result.metrics[0].verdict, Verdict::NoBaseline);
        assert!(!result.has_regression());
    }

    #[test]
    fn too_few_samples_falls_back_to_thresholded() {
        // Fewer than MIN_SAMPLES samples: a rank test is meaningless, so the
        // regime is thresholded and the effect-size threshold decides. A 2x
        // slowdown clears the 2% threshold and regresses.
        let base = Metric::distribution("wall_clock", "ns", true, vec![100.0, 101.0, 102.0]);
        let cand = Metric::distribution("wall_clock", "ns", true, vec![200.0, 201.0, 202.0]);
        let result = compare(&run_with(base), &run_with(cand), CompareConfig::default());
        assert_eq!(result.metrics[0].regime, Regime::Thresholded);
        assert_eq!(result.metrics[0].verdict, Verdict::Regression);
        assert!(result.metrics[0].p_value.is_none(), "thresholded regime makes no significance claim");
    }

    #[test]
    fn zero_spread_samples_use_threshold_not_exact_compare() {
        // RSS-shaped data: many identical samples within each recording. A change
        // below the threshold must NOT regress (the bug an exact compare caused),
        // while a change above it must.
        let base = Metric::distribution("max_rss", "bytes", true, vec![1000.0; 10]);
        let small = Metric::distribution("max_rss", "bytes", true, vec![1010.0; 10]);
        let big = Metric::distribution("max_rss", "bytes", true, vec![1100.0; 10]);

        let unchanged = compare(&run_with(base.clone()), &run_with(small), CompareConfig::default());
        assert_eq!(unchanged.metrics[0].regime, Regime::Thresholded);
        assert_eq!(unchanged.metrics[0].verdict, Verdict::Unchanged, "1% RSS wobble is below the 2% threshold");

        let regressed = compare(&run_with(base), &run_with(big), CompareConfig::default());
        assert_eq!(regressed.metrics[0].verdict, Verdict::Regression, "10% RSS growth clears the threshold");
    }

    #[test]
    fn mann_whitney_separated_samples_are_significant() {
        let a: Vec<f64> = (0..10).map(f64::from).collect();
        let b: Vec<f64> = (100..110).map(f64::from).collect();
        assert!(mann_whitney_u_pvalue(&a, &b) < 0.001);
    }

    #[test]
    fn mann_whitney_identical_samples_are_not_significant() {
        let a = vec![5.0; 10];
        let b = vec![5.0; 10];
        assert!((mann_whitney_u_pvalue(&a, &b) - 1.0).abs() < 1e-12);
    }

    #[test]
    fn erf_matches_known_values() {
        assert!((erf(0.0)).abs() < 1e-6);
        assert!((erf(1.0) - 0.842_700_79).abs() < 1e-5);
        assert!((erf(-1.0) + 0.842_700_79).abs() < 1e-5);
    }
}
