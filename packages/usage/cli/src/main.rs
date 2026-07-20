//! `ix-usage`: agent-first CLI over the local usage telemetry store
//! (index#3802).
//!
//! Every read subcommand compacts the spool first, so callers always see
//! current data, and every listing supports `--json`. The upload path sends
//! aggregated counts only; see `ix_usage_core::payload` for the invariant.

use anyhow::Context as _;
use clap::{Parser, Subcommand};
use ix_usage_core::{consent, paths, payload, spool, store};
use rusqlite::Connection;
use serde::Serialize;
use std::path::PathBuf;

#[derive(Parser)]
#[command(
    name = "ix-usage",
    version,
    about = "Local ix usage telemetry: consent, captured failures, uploads"
)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Show consent state and store locations.
    Status {
        /// Machine-readable output.
        #[arg(long)]
        json: bool,
    },
    /// Enable uploads (writes the config file).
    On,
    /// Disable uploads (writes the config file). Local recording continues.
    Off,
    /// Print the exact JSON payload the next upload would send.
    Show,
    /// List captured failures, newest first.
    Errors {
        /// Machine-readable output.
        #[arg(long)]
        json: bool,
        /// Maximum rows.
        #[arg(long, default_value_t = 20)]
        limit: u32,
    },
    /// Show one captured failure by id.
    Error {
        /// Row id from `ix-usage errors`.
        id: i64,
        /// Machine-readable output.
        #[arg(long)]
        json: bool,
    },
    /// Fold pending spool records into the database.
    Compact,
    /// Upload aggregated counts to the collector.
    Upload {
        /// Only upload when consent allows and the last upload is stale;
        /// exit quietly otherwise (used by the wrapper's detached kick).
        #[arg(long)]
        if_due: bool,
        /// Build and print the payload without sending.
        #[arg(long)]
        dry_run: bool,
    },
}

/// Minimum spacing between uploads.
const UPLOAD_INTERVAL_MS: i64 = 24 * 3600 * 1000;

fn main() -> anyhow::Result<()> {
    match Cli::parse().command {
        Command::Status { json } => status(json),
        Command::On => set_enabled(true),
        Command::Off => set_enabled(false),
        Command::Show => show(),
        Command::Errors { json, limit } => errors(json, limit),
        Command::Error { id, json } => error_by_id(id, json),
        Command::Compact => compact_cmd(),
        Command::Upload { if_due, dry_run } => upload(if_due, dry_run),
    }
}

fn state_dir() -> anyhow::Result<PathBuf> {
    paths::state_dir().context("no home directory: usage telemetry has no state here")
}

/// Compact, then open the database (the read-side convention: readers always
/// see current data).
fn compacted_db() -> anyhow::Result<Connection> {
    let state = state_dir()?;
    store::compact(&state)?;
    store::open(&state.join("usage.db"))
}

#[derive(Serialize)]
struct Status {
    upload_enabled: bool,
    source: &'static str,
    ci: bool,
    config_path: Option<PathBuf>,
    state_dir: Option<PathBuf>,
    last_upload_ms: Option<i64>,
    install_id: Option<String>,
}

fn read_status() -> anyhow::Result<Status> {
    let consent = consent::resolve()?;
    let mut last_upload_ms = None;
    let mut install_id = None;
    if let Some(db) = paths::db_path().filter(|db| db.exists()) {
        let conn = store::open(&db)?;
        last_upload_ms = store::meta_get(&conn, "last_upload_ms")?
            .map(|value| value.parse::<i64>())
            .transpose()?;
        install_id = store::meta_get(&conn, "install_id")?;
    }
    Ok(Status {
        upload_enabled: consent.upload,
        source: consent.source.as_str(),
        ci: consent::is_ci(),
        config_path: paths::config_path(),
        state_dir: paths::state_dir(),
        last_upload_ms,
        install_id,
    })
}

fn status(json: bool) -> anyhow::Result<()> {
    let status = read_status()?;
    if json {
        println!("{}", serde_json::to_string_pretty(&status)?);
        return Ok(());
    }
    println!(
        "uploads:   {} (decided by {})",
        on_off(status.upload_enabled),
        status.source
    );
    println!("ci:        {}", status.ci);
    match &status.config_path {
        Some(path) => println!("config:    {}", path.display()),
        None => println!("config:    (no home directory)"),
    }
    match &status.state_dir {
        Some(dir) => println!("state:     {}", dir.display()),
        None => println!("state:     (no home directory)"),
    }
    match status.last_upload_ms {
        Some(ts) => println!("last sent: {ts} (unix ms)"),
        None => println!("last sent: never"),
    }
    Ok(())
}

const fn on_off(enabled: bool) -> &'static str {
    if enabled { "on" } else { "off" }
}

fn set_enabled(enabled: bool) -> anyhow::Result<()> {
    let mut config = consent::read_config()?.unwrap_or_default();
    config.enabled = Some(enabled);
    let path = consent::write_config(&config)?;
    println!("uploads {} ({})", on_off(enabled), path.display());
    Ok(())
}

fn show() -> anyhow::Result<()> {
    let conn = compacted_db()?;
    let since = payload::since_day(payload::REPORT_WINDOW_DAYS)?;
    let report = payload::build_report(&conn, &since, consent::is_ci())?;
    println!("{}", serde_json::to_string_pretty(&report)?);
    Ok(())
}

/// A captured failure as stored locally. Never uploaded.
#[derive(Serialize)]
struct ErrorRow {
    id: i64,
    ts_ms: i64,
    pkg: String,
    version: String,
    exit_code: Option<i32>,
    argv: Option<Vec<String>>,
    cwd: Option<String>,
    duration_ms: Option<i64>,
    reported_ref: Option<String>,
}

fn map_error_row(row: &rusqlite::Row) -> rusqlite::Result<ErrorRow> {
    let argv_json: Option<String> = row.get(5)?;
    Ok(ErrorRow {
        id: row.get(0)?,
        ts_ms: row.get(1)?,
        pkg: row.get(2)?,
        version: row.get(3)?,
        exit_code: row.get(4)?,
        argv: argv_json.and_then(|text| serde_json::from_str(&text).ok()),
        cwd: row.get(6)?,
        duration_ms: row.get(7)?,
        reported_ref: row.get(8)?,
    })
}

const ERROR_COLUMNS: &str =
    "id, ts_ms, pkg, version, exit_code, argv, cwd, duration_ms, reported_ref";

fn errors(json: bool, limit: u32) -> anyhow::Result<()> {
    let conn = compacted_db()?;
    let mut stmt = conn.prepare(&format!(
        "SELECT {ERROR_COLUMNS} FROM errors ORDER BY id DESC LIMIT ?1"
    ))?;
    let rows = stmt
        .query_map([limit], |row| map_error_row(row))?
        .collect::<Result<Vec<_>, _>>()?;
    if json {
        println!("{}", serde_json::to_string_pretty(&rows)?);
        return Ok(());
    }
    if rows.is_empty() {
        println!("no captured failures");
        return Ok(());
    }
    for row in &rows {
        let exit = row
            .exit_code
            .map_or_else(|| "?".to_owned(), |code| code.to_string());
        println!(
            "{}  {}  {}@{}  exit {exit}",
            row.id, row.ts_ms, row.pkg, row.version
        );
    }
    Ok(())
}

fn error_by_id(id: i64, json: bool) -> anyhow::Result<()> {
    let conn = compacted_db()?;
    let row = conn
        .query_row(
            &format!("SELECT {ERROR_COLUMNS} FROM errors WHERE id = ?1"),
            [id],
            |row| map_error_row(row),
        )
        .with_context(|| format!("no captured failure with id {id}"))?;
    if json {
        println!("{}", serde_json::to_string_pretty(&row)?);
        return Ok(());
    }
    println!("{}", serde_json::to_string_pretty(&row)?);
    Ok(())
}

fn compact_cmd() -> anyhow::Result<()> {
    let stats = store::compact(&state_dir()?)?;
    println!(
        "folded {} record(s), skipped {}",
        stats.folded, stats.skipped
    );
    Ok(())
}

fn endpoint(config: Option<&consent::Config>) -> String {
    if let Ok(endpoint) = std::env::var("IX_USAGE_ENDPOINT") {
        return endpoint;
    }
    config
        .and_then(|config| config.endpoint.clone())
        .unwrap_or_else(|| payload::DEFAULT_ENDPOINT.to_owned())
}

fn upload(if_due: bool, dry_run: bool) -> anyhow::Result<()> {
    let config = consent::read_config()?;
    let resolved = consent::resolve()?;
    if !resolved.upload {
        if if_due {
            return Ok(());
        }
        anyhow::bail!("uploads are off (decided by {})", resolved.source.as_str());
    }

    let conn = compacted_db()?;
    let now = spool::now_ms()?;
    if if_due {
        let last = store::meta_get(&conn, "last_upload_ms")?
            .map(|value| value.parse::<i64>())
            .transpose()?;
        if last.is_some_and(|last| now - last < UPLOAD_INTERVAL_MS) {
            return Ok(());
        }
    }

    let since = payload::since_day(payload::REPORT_WINDOW_DAYS)?;
    let report = payload::build_report(&conn, &since, consent::is_ci())?;
    if dry_run {
        println!("{}", serde_json::to_string_pretty(&report)?);
        return Ok(());
    }
    if report.counts.is_empty() {
        println!("nothing to upload");
        return Ok(());
    }

    let endpoint = endpoint(config.as_ref());
    ureq::post(&endpoint)
        .send_json(&report)
        .with_context(|| format!("posting usage report to {endpoint}"))?;
    store::meta_set(&conn, "last_upload_ms", &now.to_string())?;
    println!(
        "uploaded {} count row(s) to {endpoint}",
        report.counts.len()
    );
    Ok(())
}
