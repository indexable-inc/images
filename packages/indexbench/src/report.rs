//! Render a [`Comparison`] for humans or machines.
//!
//! The human table mirrors criterion/tango: each metric shows its candidate
//! value, the baseline, the percent delta versus baseline, a significance marker
//! for distributional metrics, and a verdict. The JSON form is the
//! [`Comparison`] serialized verbatim, for a CI job or the (stubbed) time-series
//! viewer to consume.

use std::fmt::Write as _;

use crate::compare::{Comparison, Regime, Verdict};

/// Render a comparison as a human-readable table.
///
/// The percent delta is the signed change in headline value, oriented so a
/// minus sign always means improvement (matching the comparator's
/// `relative_change`). The marker column reads `***` for a significant
/// distributional change, `=` for deterministic, and a blank for a
/// not-significant distributional metric.
#[must_use]
pub fn human_table(comparison: &Comparison) -> String {
    let mut out = String::new();
    let _ = writeln!(out, "{}/{}", comparison.suite, comparison.bench);

    let header = format!("  {:<16} {:>14} {:>14} {:>9}  {:<4} {}", "metric", "candidate", "baseline", "delta", "sig", "verdict");
    let _ = writeln!(out, "{header}");
    let _ = writeln!(out, "  {}", "-".repeat(header.len() - 2));

    for metric in &comparison.metrics {
        let baseline = metric.baseline_value.map_or_else(|| "—".to_owned(), |v| format!("{v:.3}"));
        let delta = metric.relative_change.map_or_else(|| "—".to_owned(), |rc| format!("{:+.2}%", rc * 100.0));
        let marker = match (metric.regime, metric.p_value) {
            // `=` exact compare, `~` threshold-only, `***` significant, blank otherwise.
            (Regime::Deterministic, _) => "=",
            (Regime::Thresholded, _) => "~",
            (Regime::Distributional, Some(p)) if p < crate::compare::DEFAULT_ALPHA => "***",
            (Regime::Distributional, _) => "",
        };
        let _ = writeln!(
            out,
            "  {:<16} {:>14.3} {:>14} {:>9}  {:<4} {}",
            truncate(&metric.name, 16),
            metric.candidate_value,
            baseline,
            delta,
            marker,
            verdict_label(metric.verdict),
        );
    }

    out
}

/// Serialize a comparison as pretty JSON.
///
/// # Errors
///
/// Returns an error when the comparison cannot be serialized, which only happens
/// on a serializer fault since all fields are plain data.
pub fn json(comparison: &Comparison) -> crate::Result<String> {
    use snafu::ResultExt;
    serde_json::to_string_pretty(comparison).context(crate::error::SerializeSnafu)
}

/// A short, color-free verdict label for the table's last column.
const fn verdict_label(verdict: Verdict) -> &'static str {
    match verdict {
        Verdict::Improvement => "improvement",
        Verdict::Regression => "REGRESSION",
        Verdict::Unchanged => "unchanged",
        Verdict::NoBaseline => "no-baseline",
    }
}

/// Truncate a string to `max` chars with an ellipsis, so a long metric name does
/// not break the column alignment.
fn truncate(value: &str, max: usize) -> String {
    if value.chars().count() <= max {
        return value.to_owned();
    }
    let kept: String = value.chars().take(max.saturating_sub(1)).collect();
    format!("{kept}…")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::compare::{CompareConfig, compare};
    use crate::schema::{Metric, Run};

    fn run_with(metrics: Vec<Metric>) -> Run {
        Run {
            suite: "self-demo".to_owned(),
            bench: "fib".to_owned(),
            metrics,
            machine_id: "m".to_owned(),
            git_commit: "c".to_owned(),
            git_dirty: false,
            timestamp_unix: 0,
        }
    }

    #[test]
    fn table_marks_a_regression_and_shows_delta() {
        let base = run_with(vec![Metric::deterministic("allocations", 10.0, "count", true)]);
        let cand = run_with(vec![Metric::deterministic("allocations", 12.0, "count", true)]);
        let comparison = compare(&base, &cand, CompareConfig::default());
        let table = human_table(&comparison);
        assert!(table.contains("allocations"));
        assert!(table.contains("REGRESSION"));
        assert!(table.contains("+20.00%"), "table should show the percent delta: {table}");
    }

    #[test]
    fn json_round_trips_to_a_value() {
        let base = run_with(vec![Metric::deterministic("allocations", 10.0, "count", true)]);
        let cand = run_with(vec![Metric::deterministic("allocations", 10.0, "count", true)]);
        let comparison = compare(&base, &cand, CompareConfig::default());
        let rendered = json(&comparison).expect("json");
        let value: serde_json::Value = serde_json::from_str(&rendered).expect("parse");
        assert_eq!(value["bench"], "fib");
    }
}
