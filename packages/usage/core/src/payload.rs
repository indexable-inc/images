//! Upload payload: aggregated counts only.
//!
//! This module is the only code the uploader calls, and it reads only the
//! `counts` and `meta` tables. Error records (argv, cwd, exit details) have
//! no code path into a [`Report`]; keep it that way, it is the privacy
//! guarantee users were shown at first run.

use rusqlite::Connection;
use serde::Serialize;

/// The default collector endpoint (`ix` fleet leader; see indexable-inc/ix).
pub const DEFAULT_ENDPOINT: &str = "https://usage.ix.dev/v1/report";

/// How many trailing days of day-buckets each report carries.
///
/// Uploads are idempotent server-side (keyed install/pkg/version/day), so
/// overlap between consecutive reports is harmless and covers offline gaps.
pub const REPORT_WINDOW_DAYS: i64 = 7;

/// One day-bucketed count row.
#[derive(Serialize, Debug, Clone, PartialEq, Eq)]
pub struct CountRow {
    /// Package id.
    pub pkg: String,
    /// Package version.
    pub version: String,
    /// `YYYY-MM-DD` (UTC).
    pub day: String,
    /// Invocations that day (non-negative; `i64` is `SQLite`'s integer type).
    pub runs: i64,
    /// Invocations that exited nonzero.
    pub failures: i64,
}

/// The wire payload `POST`ed to the collector.
#[derive(Serialize, Debug, Clone, PartialEq, Eq)]
pub struct Report {
    /// Payload schema version.
    pub v: u32,
    /// Random per-install UUID (minted locally, meaningless elsewhere).
    pub install: String,
    /// `std::env::consts::OS`.
    pub os: &'static str,
    /// `std::env::consts::ARCH`.
    pub arch: &'static str,
    /// Whether this environment looks like CI (`CI` env truthy).
    pub ci: bool,
    /// Day-bucketed counts.
    pub counts: Vec<CountRow>,
}

/// Build the next report from counts on or after `since_day` (inclusive,
/// `YYYY-MM-DD`).
///
/// # Errors
/// `SQLite` failures.
pub fn build_report(conn: &Connection, since_day: &str, ci: bool) -> anyhow::Result<Report> {
    let install = crate::store::install_id(conn)?;
    let mut stmt = conn.prepare(
        "SELECT pkg, version, day, invocations, nonzero_exits
         FROM counts WHERE day >= ?1 ORDER BY day, pkg, version",
    )?;
    let counts = stmt
        .query_map([since_day], |row| {
            Ok(CountRow {
                pkg: row.get(0)?,
                version: row.get(1)?,
                day: row.get(2)?,
                runs: row.get(3)?,
                failures: row.get(4)?,
            })
        })?
        .collect::<Result<Vec<_>, _>>()?;
    Ok(Report {
        v: 1,
        install,
        os: std::env::consts::OS,
        arch: std::env::consts::ARCH,
        ci,
        counts,
    })
}

/// `YYYY-MM-DD` (UTC) for `days_back` days before now; the default report
/// window lower bound.
///
/// # Errors
/// Clock failures (before epoch) or unrepresentable timestamps.
pub fn since_day(days_back: i64) -> anyhow::Result<String> {
    let now = crate::spool::now_ms()?;
    let ts = now - days_back * 86_400_000;
    crate::store::day_from_ts_ms(ts)
        .ok_or_else(|| anyhow::anyhow!("timestamp {ts} maps to no calendar day"))
}

#[cfg(test)]
mod tests {
    use super::build_report;
    use crate::spool::{Record, append_at};
    use crate::store::{compact, open};

    #[test]
    fn report_carries_counts_and_never_argv() {
        let dir = tempfile::tempdir().expect("tempdir");
        let record = Record {
            ts_ms: 1_700_000_000_000,
            pkg: "demo".to_owned(),
            version: "1.0".to_owned(),
            exit: Some(9),
            duration_ms: Some(2),
            argv: Some(vec!["demo".to_owned(), "--secret-token=hunter2".to_owned()]),
            cwd: Some("/home/user/secret-project".to_owned()),
        };
        append_at(&dir.path().join("usage.spool"), &record).expect("append");
        compact(dir.path()).expect("compact");

        let conn = open(&dir.path().join("usage.db")).expect("open");
        let report = build_report(&conn, "1970-01-01", false).expect("build");
        assert_eq!(report.counts.len(), 1);
        assert_eq!(report.counts[0].runs, 1);
        assert_eq!(report.counts[0].failures, 1);

        let wire = serde_json::to_string(&report).expect("serialize");
        assert!(!wire.contains("argv"), "payload must never mention argv");
        assert!(
            !wire.contains("hunter2"),
            "payload must never carry argv contents"
        );
        assert!(
            !wire.contains("secret-project"),
            "payload must never carry cwd"
        );
    }

    #[test]
    fn since_day_filters_old_buckets() {
        let dir = tempfile::tempdir().expect("tempdir");
        let conn = open(&dir.path().join("usage.db")).expect("open");
        conn.execute_batch(
            "INSERT INTO counts VALUES ('old', '1', '2000-01-01', 5, 0);
             INSERT INTO counts VALUES ('new', '1', '2999-01-01', 2, 1);",
        )
        .expect("seed");
        let report = build_report(&conn, "2500-01-01", true).expect("build");
        assert_eq!(report.counts.len(), 1);
        assert_eq!(report.counts[0].pkg, "new");
        assert!(report.ci);
    }
}
