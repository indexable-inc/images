//! The result schema: [`Metric`] and [`Run`].
//!
//! The framework is built around a [`Metric`], not around time. A metric is any
//! number with a unit and a direction: wall-clock nanoseconds, peak RSS bytes,
//! allocation counts, a match rate, NAR bytes, force-resolve steps. Harnesses
//! produce metrics; the comparator and reporter consume them. Nothing in this
//! module knows whether a number came from a timer or a custom `@bench` line, so
//! adding a new kind of measurement never touches the schema.
//!
//! A [`Run`] is one execution of one bench on one machine at one commit. Runs
//! are the unit the [history store](crate::store) appends and the
//! [comparator](crate::compare) diffs. The JSON/JSONL form is stable and is the
//! contract consumers serialize against.

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

/// One measured number with its unit and improvement direction.
///
/// `samples` is `Some` for distributional metrics (a timed loop that recorded
/// per-iteration values), and `None` for deterministic metrics (an allocation
/// count, a one-shot byte size). The comparator branches on this: distributions
/// get a significance test, deterministic values get an exact compare. `value`
/// is the headline number a reporter shows; for a distribution it is the
/// representative central tendency (the median), kept alongside the raw
/// `samples` so a consumer never has to recompute it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Metric {
    /// Stable identifier within a bench, e.g. `wall_clock`, `max_rss`,
    /// `allocations`, `match_rate`.
    pub name: String,
    /// The headline value. For a distribution this is the median of `samples`.
    pub value: f64,
    /// Unit string for display only, e.g. `ns`, `bytes`, `count`, `ratio`.
    pub unit: String,
    /// Whether a smaller value is an improvement. Time/RSS/allocations are
    /// `true`; a match rate or throughput is `false`.
    pub lower_is_better: bool,
    /// Per-iteration raw values when the metric is distributional. Absent for
    /// deterministic metrics. The presence of this field, not the metric name,
    /// decides whether the comparator runs a statistical test.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub samples: Option<Vec<f64>>,
}

impl Metric {
    /// Build a deterministic metric (no distribution).
    #[must_use]
    pub fn deterministic(
        name: impl Into<String>,
        value: f64,
        unit: impl Into<String>,
        lower_is_better: bool,
    ) -> Self {
        Self {
            name: name.into(),
            value,
            unit: unit.into(),
            lower_is_better,
            samples: None,
        }
    }

    /// Build a distributional metric from raw `samples`, setting `value` to the
    /// median so the headline number is robust to outliers.
    ///
    /// An empty `samples` slice yields a metric whose `value` is `0.0` and whose
    /// `samples` is an empty vector; the comparator treats too-few-samples as
    /// deterministic, so this stays well defined rather than panicking.
    #[must_use]
    pub fn distribution(
        name: impl Into<String>,
        unit: impl Into<String>,
        lower_is_better: bool,
        samples: Vec<f64>,
    ) -> Self {
        let value = median(&samples);
        Self {
            name: name.into(),
            value,
            unit: unit.into(),
            lower_is_better,
            samples: Some(samples),
        }
    }
}

/// One execution of one bench, the unit the history store appends.
///
/// `(machine_id, git_commit, timestamp_unix)` is the natural key the store sorts
/// and the comparator keys "previous run, same machine" on. `git_dirty` is
/// recorded so a comparison against a dirty tree can be flagged rather than
/// silently trusted.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Run {
    /// Logical group of benches, e.g. `search` or `self-demo`.
    pub suite: String,
    /// Bench name within the suite.
    pub bench: String,
    /// Every metric this bench produced.
    pub metrics: Vec<Metric>,
    /// Stable per-machine identifier (see [`machine_id`]).
    pub machine_id: String,
    /// The commit the bench ran against, or `unknown` when not in a repo.
    pub git_commit: String,
    /// Whether the working tree had uncommitted changes at run time.
    pub git_dirty: bool,
    /// Wall-clock time the run finished, seconds since the Unix epoch.
    pub timestamp_unix: i64,
}

impl Run {
    /// Find a metric by name. Returns `None` when the bench did not emit it,
    /// which the comparator reports as a missing-metric rather than guessing.
    #[must_use]
    pub fn metric(&self, name: &str) -> Option<&Metric> {
        self.metrics.iter().find(|m| m.name == name)
    }
}

/// Derive a stable machine identifier from the hostname and CPU model.
///
/// The id is stable across reboots and runs on one box (so the "previous run,
/// same machine" baseline lines up) but differs between machines with different
/// CPUs (so timing baselines are not compared across hardware). It hashes
/// `hostname` plus the first `model name` line from `/proc/cpuinfo`, falling back
/// to the hostname alone off Linux. The hash is truncated to 16 hex chars:
/// collision risk across a fleet is negligible and a short id keeps the JSON
/// readable.
///
/// # Errors
///
/// Returns [`Error::Hostname`](crate::Error::Hostname) when the hostname cannot
/// be read.
pub fn machine_id() -> crate::Result<String> {
    use snafu::ResultExt;

    let host = nix::unistd::gethostname().context(crate::error::HostnameSnafu)?;
    let host = host.to_string_lossy();

    let cpu = cpu_model().unwrap_or_default();

    let mut hasher = Sha256::new();
    hasher.update(host.as_bytes());
    hasher.update(b"\0");
    hasher.update(cpu.as_bytes());
    let digest = hasher.finalize();

    Ok(hex16(&digest))
}

/// First `model name` value from `/proc/cpuinfo`, or `None` off Linux / on read
/// failure. A missing model only widens the id's scope to the hostname, which is
/// still stable per machine, so this never propagates an error.
fn cpu_model() -> Option<String> {
    let contents = std::fs::read_to_string("/proc/cpuinfo").ok()?;
    contents
        .lines()
        .find_map(|line| line.strip_prefix("model name"))
        .and_then(|rest| rest.split_once(':'))
        .map(|(_, value)| value.trim().to_owned())
}

/// Lowercase-hex the first 8 bytes of a digest (16 chars).
fn hex16(digest: &[u8]) -> String {
    use std::fmt::Write;
    digest
        .iter()
        .take(8)
        .fold(String::with_capacity(16), |mut acc, byte| {
            // `write!` into a String is infallible; the buffer never errors.
            let _ = write!(acc, "{byte:02x}");
            acc
        })
}

/// Median of a sample slice.
///
/// Returns `0.0` for an empty slice so a degenerate distribution stays well
/// defined. The slice is cloned and sorted; callers pass small per-iteration
/// vectors so the copy is cheap relative to running the bench itself.
#[must_use]
pub fn median(samples: &[f64]) -> f64 {
    if samples.is_empty() {
        return 0.0;
    }
    let mut sorted = samples.to_vec();
    // `total_cmp` gives a total order even with NaNs, so a stray NaN sorts to
    // the end rather than poisoning the comparison and panicking.
    sorted.sort_by(f64::total_cmp);
    let mid = sorted.len() / 2;
    if sorted.len().is_multiple_of(2) {
        f64::midpoint(sorted[mid - 1], sorted[mid])
    } else {
        sorted[mid]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Exact-up-to-rounding float equality for test assertions. The values under
    /// test are exact by construction (medians of small integers); the epsilon
    /// only sidesteps `clippy::float_cmp` without weakening the check.
    fn close(actual: f64, expected: f64) -> bool {
        (actual - expected).abs() < 1e-9
    }

    #[test]
    fn median_handles_even_and_odd() {
        assert!(close(median(&[1.0, 2.0, 3.0]), 2.0));
        assert!(close(median(&[1.0, 2.0, 3.0, 4.0]), 2.5));
        assert!(close(median(&[]), 0.0));
    }

    #[test]
    fn distribution_metric_uses_median_as_value() {
        let metric = Metric::distribution("wall_clock", "ns", true, vec![10.0, 30.0, 20.0]);
        assert!(close(metric.value, 20.0));
        assert_eq!(
            metric.samples.as_deref(),
            Some([10.0, 30.0, 20.0].as_slice())
        );
    }

    #[test]
    fn run_round_trips_through_json() {
        let run = Run {
            suite: "self-demo".to_owned(),
            bench: "fib".to_owned(),
            metrics: vec![
                Metric::distribution("wall_clock", "ns", true, vec![100.0, 110.0, 105.0]),
                Metric::deterministic("allocations", 3.0, "count", true),
            ],
            machine_id: "abc123".to_owned(),
            git_commit: "deadbeef".to_owned(),
            git_dirty: false,
            timestamp_unix: 1_700_000_000,
        };

        let json = serde_json::to_string(&run).expect("serialize run");
        let back: Run = serde_json::from_str(&json).expect("deserialize run");
        assert_eq!(run, back);
    }

    #[test]
    fn deterministic_metric_omits_samples_in_json() {
        let metric = Metric::deterministic("allocations", 7.0, "count", true);
        let json = serde_json::to_string(&metric).expect("serialize metric");
        assert!(
            !json.contains("samples"),
            "deterministic metric should not serialize a null samples field: {json}"
        );
    }

    #[test]
    fn machine_id_is_stable_and_hex() {
        let first = machine_id().expect("machine id");
        let second = machine_id().expect("machine id");
        assert_eq!(first, second, "machine id must be stable across calls");
        assert_eq!(first.len(), 16);
        assert!(first.chars().all(|c| c.is_ascii_hexdigit()));
    }
}
