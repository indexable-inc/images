//! Fold phase files into a report.
//!
//! Totals, top hosts, and longest connections per phase, rendered as
//! Markdown locally and as constrained JSON for the trusted CI comment job
//! (which re-renders with its own jq; see check.yml).

use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::fs;
use std::path::Path;

use color_eyre::eyre::{Context, Result};
use serde::{Deserialize, Serialize};

use crate::proxy::Connection;

/// One `net-trace run` invocation: the wrapped command plus every connection
/// its process tree opened through the proxy.
#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Phase {
    pub label: String,
    pub cmd: Vec<String>,
    /// Unix epoch milliseconds when the phase started; orders phases.
    pub started_at_ms: u64,
    pub wall_ms: u64,
    /// None when the child died to a signal.
    pub exit_code: Option<i32>,
    pub connections: Vec<Connection>,
}

/// Everything the report needs, aggregated per phase.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Summary {
    pub phases: Vec<PhaseSummary>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PhaseSummary {
    pub label: String,
    pub wall_ms: u64,
    /// Union of connection intervals: network time the phase actually waited,
    /// with concurrent connections counted once.
    pub network_wall_ms: u64,
    pub connections: u64,
    pub failed: u64,
    pub bytes_up: u64,
    pub bytes_down: u64,
    pub top_hosts: Vec<HostStat>,
    pub longest: Vec<Connection>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct HostStat {
    pub host: String,
    pub port: u16,
    pub connections: u64,
    pub connected_ms: u64,
    pub bytes_up: u64,
    pub bytes_down: u64,
    pub failed: u64,
}

/// Hosts and connections kept per phase in the summary; the raw phase files
/// remain the full record.
const TOP: usize = 8;

/// Load every `*.json` phase file in `dir`, ordered by phase start time.
///
/// # Errors
/// Fails on unreadable directory or a file that is not a phase JSON.
pub fn load(dir: &Path) -> Result<Vec<Phase>> {
    let mut phases = Vec::new();
    for entry in fs::read_dir(dir).wrap_err_with(|| format!("read {}", dir.display()))? {
        let path = entry.wrap_err("read phase dir entry")?.path();
        if path.extension().is_none_or(|extension| extension != "json") {
            continue;
        }
        let text =
            fs::read_to_string(&path).wrap_err_with(|| format!("read {}", path.display()))?;
        phases.push(
            serde_json::from_str(&text).wrap_err_with(|| format!("parse {}", path.display()))?,
        );
    }
    phases.sort_by_key(|phase: &Phase| phase.started_at_ms);
    Ok(phases)
}

#[must_use]
pub fn summarize(phases: &[Phase]) -> Summary {
    Summary {
        phases: phases.iter().map(summarize_phase).collect(),
    }
}

fn summarize_phase(phase: &Phase) -> PhaseSummary {
    let mut hosts: BTreeMap<(String, u16), HostStat> = BTreeMap::new();
    for connection in &phase.connections {
        let stat = hosts
            .entry((connection.host.clone(), connection.port))
            .or_insert_with(|| HostStat {
                host: connection.host.clone(),
                port: connection.port,
                connections: 0,
                connected_ms: 0,
                bytes_up: 0,
                bytes_down: 0,
                failed: 0,
            });
        stat.connections += 1;
        stat.connected_ms += connection.dur_ms;
        stat.bytes_up += connection.bytes_up;
        stat.bytes_down += connection.bytes_down;
        stat.failed += u64::from(connection.failed);
    }
    let mut top_hosts: Vec<HostStat> = hosts.into_values().collect();
    top_hosts.sort_by_key(|stat| std::cmp::Reverse(stat.connected_ms));
    top_hosts.truncate(TOP);

    let mut longest = phase.connections.clone();
    longest.sort_by_key(|connection| std::cmp::Reverse(connection.dur_ms));
    longest.truncate(TOP);

    PhaseSummary {
        label: phase.label.clone(),
        wall_ms: phase.wall_ms,
        network_wall_ms: interval_union_ms(&phase.connections),
        connections: phase.connections.len() as u64,
        failed: phase.connections.iter().filter(|connection| connection.failed).count() as u64,
        bytes_up: phase.connections.iter().map(|connection| connection.bytes_up).sum(),
        bytes_down: phase.connections.iter().map(|connection| connection.bytes_down).sum(),
        top_hosts,
        longest,
    }
}

/// Total time at least one connection was open: a sweep over sorted
/// [start, start + duration) intervals, merging overlaps.
fn interval_union_ms(connections: &[Connection]) -> u64 {
    let mut intervals: Vec<Interval> = connections
        .iter()
        .map(|connection| Interval {
            start: connection.start_ms,
            end: connection.start_ms + connection.dur_ms,
        })
        .collect();
    intervals.sort_by_key(|interval| interval.start);
    let mut total = 0_u64;
    let mut current: Option<Interval> = None;
    for interval in intervals {
        match &mut current {
            Some(open) if interval.start <= open.end => open.end = open.end.max(interval.end),
            _ => {
                if let Some(open) = current.take() {
                    total += open.end - open.start;
                }
                current = Some(interval);
            }
        }
    }
    if let Some(open) = current {
        total += open.end - open.start;
    }
    total
}

struct Interval {
    start: u64,
    end: u64,
}

/// `write!` into a `String` cannot fail; keep call sites clean.
fn write_fmt(out: &mut String, args: std::fmt::Arguments<'_>) {
    out.write_fmt(args).expect("String write cannot fail");
}

/// Render the human report. The sticky-comment marker comes first so the
/// trusted job's jq render and this one key the same comment.
#[must_use]
pub fn markdown(summary: &Summary) -> String {
    let mut out = String::from("<!-- net-trace -->\n### Client-side network during CI\n\n");
    if summary.phases.is_empty() {
        out.push_str("No phases recorded.\n");
        return out;
    }
    out.push_str("| phase | wall | network wall | conns | failed | down | up |\n");
    out.push_str("| --- | --- | --- | --- | --- | --- | --- |\n");
    for phase in &summary.phases {
        write_fmt(&mut out, format_args!(
            "| {} | {} | {} | {} | {} | {} | {} |\n",
            phase.label,
            fmt_ms(phase.wall_ms),
            fmt_ms(phase.network_wall_ms),
            phase.connections,
            phase.failed,
            fmt_bytes(phase.bytes_down),
            fmt_bytes(phase.bytes_up),
        ));
    }
    for phase in &summary.phases {
        if phase.top_hosts.is_empty() {
            continue;
        }
        write_fmt(&mut out, format_args!("\n**{}: top hosts**\n\n", phase.label));
        out.push_str("| host | conns | time | down | up |\n| --- | --- | --- | --- | --- |\n");
        for stat in &phase.top_hosts {
            write_fmt(&mut out, format_args!(
                "| {}:{} | {} | {} | {} | {} |\n",
                stat.host,
                stat.port,
                stat.connections,
                fmt_ms(stat.connected_ms),
                fmt_bytes(stat.bytes_down),
                fmt_bytes(stat.bytes_up),
            ));
        }
    }
    if let Some(waterfall) = waterfall(summary) {
        out.push_str("\n```text\n");
        out.push_str(&waterfall);
        out.push_str("```\n");
    }
    out.push_str(
        "\n<sub>Client-side connections only (proxy env): eval fetches, gh, git. \
         Daemon-side substitutions and fixed-output builders are not visible here.</sub>\n",
    );
    out
}

/// Longest connections across all phases as scaled text bars.
fn waterfall(summary: &Summary) -> Option<String> {
    let mut rows: Vec<WaterfallRow> = summary
        .phases
        .iter()
        .flat_map(|phase| {
            phase.longest.iter().map(|connection| WaterfallRow {
                phase: phase.label.clone(),
                connection: connection.clone(),
            })
        })
        .collect();
    rows.sort_by_key(|row| std::cmp::Reverse(row.connection.dur_ms));
    rows.truncate(TOP);
    let max = rows.first().map(|row| row.connection.dur_ms.max(1))?;
    let mut out = String::new();
    for row in &rows {
        let width = usize::try_from(row.connection.dur_ms * 24 / max).unwrap_or(24);
        write_fmt(&mut out, format_args!(
            "{:<24} {:>9} {} {} [{}]{}\n",
            format!("{}:{}", row.connection.host, row.connection.port),
            format!("+{}", fmt_ms(row.connection.start_ms)),
            "#".repeat(width.max(1)),
            fmt_ms(row.connection.dur_ms),
            row.phase,
            if row.connection.failed { " FAILED" } else { "" },
        ));
    }
    Some(out)
}

struct WaterfallRow {
    phase: String,
    connection: Connection,
}

fn fmt_ms(ms: u64) -> String {
    if ms < 1000 {
        format!("{ms}ms")
    } else if ms < 60_000 {
        format!("{:.1}s", ms_to_f64(ms) / 1000.0)
    } else {
        format!("{:.1}m", ms_to_f64(ms) / 60_000.0)
    }
}

fn fmt_bytes(bytes: u64) -> String {
    if bytes < 1024 {
        format!("{bytes}B")
    } else if bytes < 1024 * 1024 {
        format!("{:.1}KiB", bytes_to_f64(bytes) / 1024.0)
    } else {
        format!("{:.1}MiB", bytes_to_f64(bytes) / (1024.0 * 1024.0))
    }
}

/// Display-only conversions: precision loss above 2^53 is irrelevant for
/// human-formatted durations and sizes.
#[allow(clippy::cast_precision_loss)]
const fn ms_to_f64(ms: u64) -> f64 {
    ms as f64
}

#[allow(clippy::cast_precision_loss)]
const fn bytes_to_f64(bytes: u64) -> f64 {
    bytes as f64
}
