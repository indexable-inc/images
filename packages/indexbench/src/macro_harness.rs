//! Out-of-process macro harness: run an external command N times and collect
//! metrics from each run.
//!
//! Every run contributes two built-in metrics — `wall_clock` (ns) measured in
//! the parent, and `max_rss` (bytes) read from `wait4`'s `getrusage` — so the
//! command needs no instrumentation to be timed and sized. On top of that, the
//! command can print **custom metrics** on its stdout/stderr in a fixed line
//! format, and the harness folds them into the same [`Run`](crate::schema::Run).
//! That line protocol is the framework's extensibility hook: a consumer reports
//! match-rate, force-steps, or NAR bytes without the framework knowing those
//! metrics exist.
//!
//! Built-in time/RSS metrics aggregate across the N runs into distributions
//! (`wall_clock` as a sample per run; `max_rss` as a sample per run). Custom
//! metrics are deterministic by default — a command that wants its custom value
//! treated as a distribution should emit it once per run and the harness will
//! collect the per-run values into samples too.

use std::process::Command;

use snafu::{ResultExt, ensure};

use crate::error::{self};
use crate::schema::Metric;

/// The structured line a benchmarked command prints to report a custom metric.
///
/// Format (whitespace-separated `key=value` after the `@bench` sentinel):
///
/// ```text
/// @bench name=<id> value=<f64> unit=<str> lower_is_better=<bool>
/// ```
///
/// `unit` and `lower_is_better` are optional; they default to `count` and
/// `true`. Anything before `@bench` on the line is ignored, so a command can
/// prefix the marker with its own log noise. Lines without the sentinel are
/// passed through untouched.
pub const SENTINEL: &str = "@bench";

/// One parsed custom-metric line.
#[derive(Debug, Clone, PartialEq)]
pub struct CustomMetric {
    /// The metric's `name` field.
    pub name: String,
    /// The metric's `value` field.
    pub value: f64,
    /// The metric's `unit` field (default `count`).
    pub unit: String,
    /// The metric's `lower_is_better` field (default `true`).
    pub lower_is_better: bool,
}

/// Parse one line for a `@bench` custom-metric record.
///
/// Returns `Ok(None)` for a line without the sentinel (the common case for
/// ordinary log output), `Ok(Some(_))` for a well-formed record, and
/// `Err(String)` describing the first malformed field for a line that has the
/// sentinel but cannot be parsed. Surfacing the error rather than silently
/// dropping a malformed marker keeps a typo in a consumer's reporting from
/// quietly losing a metric.
///
/// # Errors
///
/// Returns the malformed-field description when a `@bench` line is missing
/// `name`/`value` or carries an unparseable `value`/`lower_is_better`.
pub fn parse_custom_metric(line: &str) -> Result<Option<CustomMetric>, String> {
    let Some(rest) = line.split_once(SENTINEL).map(|(_, tail)| tail) else {
        return Ok(None);
    };

    let mut name = None;
    let mut value = None;
    let mut unit = String::from("count");
    let mut lower_is_better = true;

    for token in rest.split_whitespace() {
        let (key, raw) = token.split_once('=').ok_or_else(|| format!("`{token}` is not key=value"))?;
        match key {
            "name" => name = Some(raw.to_owned()),
            "value" => value = Some(raw.parse::<f64>().map_err(|err| format!("value `{raw}`: {err}"))?),
            "unit" => raw.clone_into(&mut unit),
            "lower_is_better" => {
                lower_is_better = raw.parse::<bool>().map_err(|err| format!("lower_is_better `{raw}`: {err}"))?;
            }
            other => return Err(format!("unknown key `{other}`")),
        }
    }

    let name = name.ok_or_else(|| "missing `name`".to_owned())?;
    let value = value.ok_or_else(|| "missing `value`".to_owned())?;

    Ok(Some(CustomMetric {
        name,
        value,
        unit,
        lower_is_better,
    }))
}

/// One run's raw measurements before aggregation across runs.
#[derive(Debug)]
struct RunSample {
    wall_clock_ns: f64,
    max_rss_bytes: f64,
    custom: Vec<CustomMetric>,
}

/// Spawn `program args...` once, wait for it, and collect time, RSS, and any
/// `@bench` custom metrics it printed.
///
/// We reap with `libc::wait4` for the child's own `rusage`.
///
/// `wait4` returns the child's `ru_maxrss` (its peak resident set size).
/// `std::process` discards `rusage`, and `getrusage(RUSAGE_CHILDREN)` accumulates
/// across every reaped child (so a later run would inherit an earlier run's
/// peak); `wait4` keeps RSS attributed per run. `ru_maxrss` is in kibibytes on
/// Linux but in bytes on the BSDs (including macOS), so [`maxrss_to_bytes`]
/// normalizes it per platform.
#[expect(
    clippy::cast_possible_wrap,
    reason = "POSIX pids are positive and well under i32::MAX, so the u32->i32 cast cannot wrap"
)]
#[expect(
    clippy::cast_precision_loss,
    reason = "nanoseconds and byte counts at bench magnitudes are far below 2^52, so the f64 metric representation is exact"
)]
fn run_once(program: &str, args: &[String]) -> crate::Result<RunSample> {
    use std::io::Read;
    use std::time::Instant;

    let mut child = Command::new(program)
        .args(args)
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .with_context(|_| error::SpawnSnafu { command: program.to_owned() })?;

    let start = Instant::now();

    // Drain stdout and stderr *concurrently* before reaping: a child that fills
    // one pipe while holding the other open would deadlock a parent that read
    // them in series (and `wait4` would then block forever on a child blocked on
    // a full pipe). stderr drains on a thread; stdout drains here; then join.
    let stdout = child.stdout.take();
    let stderr = child.stderr.take();
    let stderr_reader = std::thread::spawn(move || {
        let mut buf = String::new();
        if let Some(mut err) = stderr {
            let _ = err.read_to_string(&mut buf);
        }
        buf
    });
    let mut stdout_buf = String::new();
    if let Some(mut out) = stdout {
        let _ = out.read_to_string(&mut stdout_buf);
    }
    let stderr_buf = stderr_reader.join().unwrap_or_default();

    let Reaped { raw_status, max_rss } = wait4(child.id() as i32, program)?;
    let wall_clock_ns = start.elapsed().as_nanos() as f64;
    let max_rss_bytes = maxrss_to_bytes(max_rss);

    // We reaped the child ourselves with `wait4`. `std::process::Child::drop`
    // does not wait, so dropping here only closes the captured pipe handles and
    // cannot double-reap the now-recycled pid.
    drop(child);

    check_exit_status(raw_status, program)?;

    let custom = collect_custom_metrics(program, &stdout_buf, &stderr_buf)?;

    Ok(RunSample {
        wall_clock_ns,
        max_rss_bytes,
        custom,
    })
}

/// What `wait4` reaped from a child: its raw wait status and peak RSS.
struct Reaped {
    /// Raw `wait` status, to be classified by [`check_exit_status`].
    raw_status: i32,
    /// `ru_maxrss` as reported by the platform (kibibytes on Linux, bytes on macOS).
    max_rss: i64,
}

/// Reap `pid` with `wait4`.
fn wait4(pid: i32, program: &str) -> crate::Result<Reaped> {
    // SAFETY: `rusage` is plain C data with no invalid bit patterns, so a zeroed
    // value is a valid initial state for `wait4` to fill.
    let mut usage: libc::rusage = unsafe { std::mem::zeroed() };
    let mut status: libc::c_int = 0;

    // SAFETY: `pid` is a child of this process (just spawned), `&mut status` and
    // `&mut usage` are valid out-pointers, and `0` flags means a blocking wait.
    let reaped = unsafe { libc::wait4(pid, &raw mut status, 0, &raw mut usage) };
    if reaped < 0 {
        let errno = std::io::Error::last_os_error();
        return error::CommandFailedSnafu {
            command: program.to_owned(),
            detail: format!("wait4 failed: {errno}"),
        }
        .fail();
    }

    Ok(Reaped {
        raw_status: status,
        max_rss: usage.ru_maxrss,
    })
}

/// Convert `wait4`'s `ru_maxrss` to bytes.
///
/// Linux reports it in kibibytes (`getrusage(2)`); the BSDs — including macOS —
/// already report bytes. We target only those two families, so a compile-time
/// `cfg` picks the right unit with no runtime branch; an unconditional `* 1024`
/// would inflate every `max_rss` 1024x on macOS.
#[cfg(target_os = "macos")]
#[expect(clippy::cast_precision_loss, reason = "byte counts at bench magnitudes are far below 2^52, so the f64 is exact")]
fn maxrss_to_bytes(ru_maxrss_bytes: i64) -> f64 {
    ru_maxrss_bytes as f64
}

/// See the macOS variant above; on Linux `ru_maxrss` is kibibytes.
#[cfg(not(target_os = "macos"))]
#[expect(clippy::cast_precision_loss, reason = "kibibyte counts at bench magnitudes are far below 2^52, so the f64 is exact")]
fn maxrss_to_bytes(ru_maxrss_kib: i64) -> f64 {
    ru_maxrss_kib as f64 * 1024.0
}

/// Classify a raw `wait` status into success or a typed failure.
fn check_exit_status(status: libc::c_int, program: &str) -> crate::Result<()> {
    // libc exposes the WIFEXITED/WEXITSTATUS/WIFSIGNALED/WTERMSIG macros as
    // functions; use them rather than re-deriving the bit layout.
    if libc::WIFEXITED(status) {
        let code = libc::WEXITSTATUS(status);
        ensure!(
            code == 0,
            error::CommandFailedSnafu {
                command: program.to_owned(),
                detail: format!("exit code {code}"),
            }
        );
        return Ok(());
    }
    if libc::WIFSIGNALED(status) {
        let signal = libc::WTERMSIG(status);
        return error::CommandFailedSnafu {
            command: program.to_owned(),
            detail: format!("killed by signal {signal}"),
        }
        .fail();
    }
    error::CommandFailedSnafu {
        command: program.to_owned(),
        detail: format!("unexpected wait status {status}"),
    }
    .fail()
}

/// Parse `@bench` lines out of a child's captured stdout and stderr.
fn collect_custom_metrics(program: &str, stdout: &str, stderr: &str) -> crate::Result<Vec<CustomMetric>> {
    let mut metrics = Vec::new();
    for line in stdout.lines().chain(stderr.lines()) {
        match parse_custom_metric(line) {
            Ok(Some(metric)) => metrics.push(metric),
            Ok(None) => {}
            Err(detail) => {
                return error::CommandFailedSnafu {
                    command: program.to_owned(),
                    detail: format!("malformed @bench line: {detail}"),
                }
                .fail();
            }
        }
    }
    Ok(metrics)
}

/// Run `program args...` `runs` times and aggregate into the final metric set.
///
/// `wall_clock` and `max_rss` become distributions over the `runs` samples so
/// the comparator can test them statistically. A custom metric reported every
/// run is also folded into a distribution (one sample per run); a custom metric
/// reported only once stays deterministic. `runs` of zero is rejected — a macro
/// bench with no runs measures nothing.
///
/// # Errors
///
/// Propagates a spawn/wait failure, a non-zero child exit, or a malformed
/// `@bench` line from any of the runs.
pub fn run_command(program: &str, args: &[String], runs: u32) -> crate::Result<Vec<Metric>> {
    ensure!(
        runs > 0,
        error::CommandFailedSnafu {
            command: program.to_owned(),
            detail: "runs must be at least 1".to_owned(),
        }
    );

    let mut samples = Vec::with_capacity(runs as usize);
    for _ in 0..runs {
        samples.push(run_once(program, args)?);
    }

    Ok(aggregate(&samples))
}

/// Fold per-run [`RunSample`]s into the final metric vector.
fn aggregate(samples: &[RunSample]) -> Vec<Metric> {
    let wall_clock: Vec<f64> = samples.iter().map(|s| s.wall_clock_ns).collect();
    let max_rss: Vec<f64> = samples.iter().map(|s| s.max_rss_bytes).collect();

    let mut metrics = vec![
        Metric::distribution("wall_clock", "ns", true, wall_clock),
        Metric::distribution("max_rss", "bytes", true, max_rss),
    ];

    metrics.extend(fold_custom(samples));
    metrics
}

/// Aggregate custom metrics across runs.
///
/// A custom metric reported in every run becomes a distribution; one reported in
/// only some runs becomes a deterministic metric from its last reported value,
/// because a partial sample set would mislead the significance test. Unit and
/// direction come from the metric's own fields, so two runs disagreeing on a
/// unit is the consumer's bug to fix, not the harness's to reconcile — we take
/// the first run's metadata.
fn fold_custom(samples: &[RunSample]) -> Vec<Metric> {
    use std::collections::BTreeMap;

    // Preserve first-seen order via a parallel key list; BTreeMap alone would
    // reorder by name and scramble the reporter's column order.
    let mut order: Vec<String> = Vec::new();
    let mut by_name: BTreeMap<String, Vec<&CustomMetric>> = BTreeMap::new();
    for sample in samples {
        for metric in &sample.custom {
            if !by_name.contains_key(&metric.name) {
                order.push(metric.name.clone());
            }
            by_name.entry(metric.name.clone()).or_default().push(metric);
        }
    }

    order
        .into_iter()
        .filter_map(|name| {
            let entries = by_name.get(&name)?;
            let first = entries.first()?;
            if entries.len() == samples.len() && samples.len() > 1 {
                let values: Vec<f64> = entries.iter().map(|m| m.value).collect();
                Some(Metric::distribution(name, first.unit.clone(), first.lower_is_better, values))
            } else {
                let last = entries.last()?;
                Some(Metric::deterministic(name, last.value, first.unit.clone(), first.lower_is_better))
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_a_full_custom_metric_line() {
        let parsed = parse_custom_metric("noise @bench name=match_rate value=0.91 unit=ratio lower_is_better=false")
            .expect("well-formed line parses")
            .expect("line carries a metric");
        assert_eq!(
            parsed,
            CustomMetric {
                name: "match_rate".to_owned(),
                value: 0.91,
                unit: "ratio".to_owned(),
                lower_is_better: false,
            }
        );
    }

    #[test]
    fn applies_unit_and_direction_defaults() {
        let parsed = parse_custom_metric("@bench name=force_steps value=42").unwrap().unwrap();
        assert_eq!(parsed.unit, "count");
        assert!(parsed.lower_is_better);
        assert!((parsed.value - 42.0).abs() < 1e-9);
    }

    #[test]
    fn ignores_lines_without_the_sentinel() {
        assert_eq!(parse_custom_metric("just a normal log line").unwrap(), None);
        assert_eq!(parse_custom_metric("name=foo value=1").unwrap(), None);
    }

    #[test]
    fn rejects_malformed_marked_lines() {
        assert!(parse_custom_metric("@bench value=1").is_err(), "missing name must error");
        assert!(parse_custom_metric("@bench name=x").is_err(), "missing value must error");
        assert!(parse_custom_metric("@bench name=x value=notanumber").is_err(), "bad float must error");
        assert!(parse_custom_metric("@bench name=x value=1 lower_is_better=maybe").is_err(), "bad bool must error");
        assert!(parse_custom_metric("@bench name=x value=1 bogus=2").is_err(), "unknown key must error");
        assert!(parse_custom_metric("@bench name=x value=1 stray").is_err(), "non key=value token must error");
    }

    #[test]
    fn folds_repeated_custom_metric_into_a_distribution() {
        let samples = vec![
            RunSample {
                wall_clock_ns: 10.0,
                max_rss_bytes: 100.0,
                custom: vec![CustomMetric {
                    name: "rate".to_owned(),
                    value: 1.0,
                    unit: "ratio".to_owned(),
                    lower_is_better: false,
                }],
            },
            RunSample {
                wall_clock_ns: 12.0,
                max_rss_bytes: 110.0,
                custom: vec![CustomMetric {
                    name: "rate".to_owned(),
                    value: 3.0,
                    unit: "ratio".to_owned(),
                    lower_is_better: false,
                }],
            },
        ];
        let metrics = aggregate(&samples);
        let rate = metrics.iter().find(|m| m.name == "rate").expect("custom metric present");
        assert_eq!(rate.samples.as_deref(), Some([1.0, 3.0].as_slice()));
        assert!(!rate.lower_is_better);
    }

    #[test]
    fn end_to_end_runs_true_and_reports_builtins() {
        // `true` is a POSIX builtin-as-binary that exits 0 immediately; it gives
        // a fast, dependency-free macro target to prove the spawn/wait/rusage
        // path produces the two built-in metrics.
        let metrics = run_command("true", &[], 3).expect("running `true` succeeds");
        assert!(metrics.iter().any(|m| m.name == "wall_clock"));
        assert!(metrics.iter().any(|m| m.name == "max_rss"));
        let wall = metrics.iter().find(|m| m.name == "wall_clock").unwrap();
        assert_eq!(wall.samples.as_ref().map(Vec::len), Some(3));
    }

    #[test]
    fn nonzero_exit_is_an_error() {
        assert!(run_command("false", &[], 1).is_err(), "a non-zero exit must surface as an error");
    }
}
