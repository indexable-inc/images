//! `indexer`: sync every configured corpus source into Mixedbread (semantic
//! search) and a durable corpus log — the full-file-overwrite Parquet log
//! and/or its successor, the Iceberg corpus lake (issue #752) — the
//! log-as-source-of-truth with the Mixedbread index as a materialized view
//! (issue #736).
//!
//! Each source is an adapter implementing [`source_meta::SourceAdapter`]. The
//! routing differs by corpus shape:
//!
//! - Per-host history (claude, codex, shell, debug) and the bulk exports (slack,
//!   linear, github, git) go to Mixedbread directly and/or the S3/R2 Parquet log
//!   (`--bucket`), reusing the `search-core` reconcile and [`sink_parquet`]. The
//!   Parquet log is the append-only source of truth: a separate consume run can
//!   rebuild the Mixedbread index from it.
//! - Code repos go direct to Mixedbread only.
//!
//! Consume mode reconciles the durable corpus log back into Mixedbread rather
//! than scanning local sources: `--from-parquet-prefix` reads the per-source
//! `data.parquet` files `sink-parquet` wrote (the consumer half of that log) and
//! replays every record into Mixedbread.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::Context as _;
use clap::Parser;
use lake_iceberg::IcebergReconciler;
use search_core::{MixedbreadStore, Store};
use sink_mixedbread::MixedbreadReconciler;
use sink_parquet::ParquetReconciler;

/// Manifest limits for code repos, matching `search-core`'s defaults.
const MAX_FILE_BYTES: u64 = 1024 * 1024;
/// Cap on new files uploaded per code sync (a runaway guard).
const MAX_FILES: usize = 10_000;
use source_meta::{Document, Reconciler as _, Source, SourceAdapter, keys};

/// How long to wait for Mixedbread to finish embedding new documents.
const INDEX_TIMEOUT: Duration = Duration::from_mins(2);

/// Sync corpus sources to Mixedbread and/or an S3/R2 parquet archive.
#[derive(Debug, Parser)]
#[command(name = "indexer", about, version)]
struct Cli {
    /// Mixedbread store name; enables the Mixedbread (semantic) sink.
    #[arg(long, env = "MXBAI_STORE")]
    mixedbread_store: Option<String>,

    /// Mixedbread API base URL.
    #[arg(long = "base-url", env = "MXBAI_BASE_URL")]
    base_url: Option<String>,

    /// Bucket for the parquet archive; enables the S3/R2 sink. Credentials come
    /// from `AWS_ACCESS_KEY_ID` / `AWS_SECRET_ACCESS_KEY`.
    #[arg(long, env = "INDEXER_BUCKET")]
    bucket: Option<String>,

    /// S3 endpoint URL; empty means AWS S3, for R2 pass the account endpoint.
    #[arg(long, env = "INDEXER_S3_ENDPOINT")]
    endpoint: Option<String>,

    /// S3 region (`auto` for R2).
    #[arg(long, env = "INDEXER_S3_REGION", default_value = "auto")]
    region: String,

    /// Key prefix under the bucket.
    #[arg(long, env = "INDEXER_PREFIX", default_value = "corpus")]
    prefix: String,

    /// Rebuild the Mixedbread index from the Parquet corpus log at this prefix;
    /// pair with --mixedbread-store and --bucket. Reads the per-source
    /// `data.parquet` files `sink-parquet` wrote (the other half of the parquet
    /// corpus log) and reconciles every record back into Mixedbread, rather than
    /// scanning local sources.
    #[arg(long, env = "INDEXER_FROM_PARQUET_PREFIX")]
    from_parquet_prefix: Option<String>,

    /// Iceberg REST catalog URI; enables the corpus-lake sink (issue #752).
    /// For R2 Data Catalog: `https://catalog.cloudflarestorage.com/<account>/<bucket>`.
    #[arg(long, env = "INDEXER_CATALOG_URI")]
    catalog_uri: Option<String>,

    /// Iceberg warehouse name (R2: `<account>_<bucket>`); required with --catalog-uri.
    #[arg(long, env = "INDEXER_WAREHOUSE")]
    warehouse: Option<String>,

    /// Bearer token for the catalog REST API.
    #[arg(long, env = "INDEXER_CATALOG_TOKEN", hide_env_values = true)]
    catalog_token: Option<String>,

    /// Rebuild the Mixedbread index from the Iceberg corpus lake; pair with
    /// --mixedbread-store and the --catalog-* flags. The lake's analog of
    /// --from-parquet-prefix.
    #[arg(long, env = "INDEXER_FROM_ICEBERG")]
    from_iceberg: bool,

    /// Apply the lake's changes since this snapshot to Mixedbread (incremental
    /// catch-up from an explicit cursor); pair with --mixedbread-store and the
    /// --catalog-* flags. Prints the snapshot to use as the next cursor.
    #[arg(long, env = "INDEXER_FROM_SNAPSHOT")]
    from_snapshot: Option<i64>,

    /// Steady-state lake consume: read the cursor from this file, apply the
    /// delta to Mixedbread, write the new cursor back. An absent file or an
    /// expired cursor falls back to a full rebuild. The fleet passes a
    /// `StateDirectory` path (cursor.json).
    #[arg(long, env = "INDEXER_CURSOR_FILE")]
    cursor_file: Option<PathBuf>,

    /// Index local agent/shell history (claude, codex, atuin) at their default
    /// paths, in addition to any explicit `--*` overrides below.
    #[arg(long)]
    local: bool,

    /// Claude Code transcript directory (default with `--local`: `~/.claude/projects`).
    #[arg(long)]
    claude_dir: Option<PathBuf>,

    /// Codex history file (default with `--local`: `~/.codex/history.jsonl`).
    #[arg(long)]
    codex_file: Option<PathBuf>,

    /// atuin history db (default with `--local`: `~/.local/share/atuin/history.db`).
    #[arg(long)]
    atuin_db: Option<PathBuf>,

    /// Slack export directory.
    #[arg(long)]
    slack_export: Option<PathBuf>,

    /// Linear export directory.
    #[arg(long)]
    linear_export: Option<PathBuf>,

    /// GitHub export directory (produced by `source-github`'s `export.sh`).
    #[arg(long)]
    github_export: Option<PathBuf>,

    /// Git repository to index commit history from (repeatable).
    #[arg(long = "git-repo")]
    git_repos: Vec<PathBuf>,

    /// Code checkout to index (content-addressed, like a bare `search`).
    /// Mixedbread only (code lives in git, not the parquet archive); repeatable.
    #[arg(long = "code-repo")]
    code_repos: Vec<PathBuf>,

    /// Index one user's local history (claude, codex, atuin) by `NAME:HOME`,
    /// repeatable. One process indexes many users — the fleet runs this as root
    /// over every account — tagging each user's records with `NAME`. Symlinked
    /// history paths are skipped so a privileged run cannot be a confused deputy.
    #[arg(long = "user", value_name = "NAME:HOME")]
    users: Vec<String>,

    /// Host name to tag `--user` records with. Defaults to the system hostname;
    /// the fleet module passes the NixOS `networking.hostName`.
    #[arg(long)]
    host: Option<String>,
}

/// The Mixedbread view for a run: the connected store, the store name, and the
/// embedding wait, passed together wherever a source may fan out to Mixedbread.
type Mixedbread<'a> = MixedbreadReconciler<'a, MixedbreadStore>;

/// Per-run tally of how many sources were indexed, soft-skipped, or failed.
///
/// `skipped` counts sources that were deliberately and visibly passed over for a
/// benign reason (e.g. an atuin db file that exists but has no `history` table
/// because that account never ran atuin). A soft skip is logged but never gates
/// the run's exit code — only `failures` does — so one uninitialized per-user
/// history db cannot degrade the whole indexing unit.
#[derive(Clone, Copy)]
struct Counts {
    indexed: usize,
    skipped: usize,
    failures: usize,
}

/// A user account to index: its name (the `user` tag) and home directory.
struct User {
    name: String,
    home: PathBuf,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();

    let store = match &cli.mixedbread_store {
        Some(_) => {
            let base_url =
                cli.base_url.clone().unwrap_or_else(|| mixedbread::DEFAULT_BASE_URL.to_owned());
            Some(MixedbreadStore::from_login(base_url).await.context("connecting to Mixedbread")?)
        }
        None => None,
    };
    let mixedbread = store
        .as_ref()
        .zip(cli.mixedbread_store.as_deref())
        .map(|(store, name)| Mixedbread { store, name, index_timeout: INDEX_TIMEOUT });

    // The consume modes replay a log into Mixedbread instead of scanning local
    // sources; exactly one log (and one read discipline) per invocation.
    let consume_modes = usize::from(cli.from_parquet_prefix.is_some())
        + usize::from(cli.from_iceberg)
        + usize::from(cli.from_snapshot.is_some())
        + usize::from(cli.cursor_file.is_some());
    anyhow::ensure!(
        consume_modes <= 1,
        "--from-parquet-prefix, --from-iceberg, --from-snapshot, and --cursor-file are mutually exclusive consume modes"
    );

    // Consume mode (Iceberg corpus lake, full): fold the lake's revision log
    // into its current per-source document sets and REPLACE each source's
    // Mixedbread records with them — the lake's replay/rebuild path. Replace,
    // not reconcile: the fold's absences are explicit tombstones, so the
    // rebuild also deletes view records the lake has let go of, including a
    // source whose records are all tombstoned. Like the parquet consume below,
    // emit and consume run as separate invocations of this binary.
    if cli.from_iceberg {
        let mixedbread =
            mixedbread.context("--from-iceberg requires --mixedbread-store (the replace target)")?;
        let Lake { catalog, ident } = connect_lake(&cli).await?;
        let state =
            lake_iceberg::read_state(catalog.as_ref(), &ident).await.context("reading the lake")?;
        return finish(run_replace(state, mixedbread).await);
    }

    // Consume mode (Iceberg corpus lake, incremental): apply the changes since
    // an explicit cursor. Stateless — the caller owns the cursor.
    if let Some(cursor) = cli.from_snapshot {
        let mixedbread =
            mixedbread.context("--from-snapshot requires --mixedbread-store (the apply target)")?;
        let Lake { catalog, ident } = connect_lake(&cli).await?;
        let mut counts = Counts { indexed: 0, skipped: 0, failures: 0 };
        let result =
            run_lake_delta(catalog.as_ref(), &ident, cursor, mixedbread).await.map(|_| ());
        record("lake-delta", result, &mut counts);
        return finish(counts);
    }

    // Consume mode (Iceberg corpus lake, steady state): the cursor lives in a
    // file; absent or expired falls back to a full rebuild. This is the
    // deployed view-catch-up invocation.
    if let Some(path) = cli.cursor_file.clone() {
        let mixedbread =
            mixedbread.context("--cursor-file requires --mixedbread-store (the apply target)")?;
        let Lake { catalog, ident } = connect_lake(&cli).await?;
        return finish(run_cursor_consume(catalog.as_ref(), &ident, &path, mixedbread).await);
    }

    // Consume mode (parquet corpus log): read the per-source `data.parquet` files
    // `sink-parquet` wrote at this prefix under `--bucket` and reconcile them back
    // into Mixedbread. Uses the SAME bucket/endpoint/region the parquet sink uses,
    // so it reads exactly what was written. Emit (scan local sources, write the
    // log) and consume (replay the log into Mixedbread) run as separate
    // invocations of this binary; consume reconciles the log rather than scanning.
    if let Some(prefix) = cli.from_parquet_prefix.clone() {
        let bucket = cli.bucket.clone().context("--from-parquet-prefix requires --bucket")?;
        let config = source_parquet::Config {
            bucket,
            endpoint: cli.endpoint.clone(),
            region: cli.region.clone(),
            prefix,
        };
        // With --catalog-uri this folds the parquet archive INTO the lake (the
        // leader's parquet->lake step under leader-funnel, issue #752);
        // otherwise it replays the archive into Mixedbread. The lake fold keeps
        // each (host, user, source) slice separate, so it cannot share
        // consume_parquet's host-flattened read.
        if cli.catalog_uri.is_some() {
            let Lake { catalog, ident } = connect_lake(&cli).await?;
            return finish(fold_parquet_into_lake(&config, catalog, &ident).await);
        }
        let mixedbread = mixedbread
            .context("--from-parquet-prefix requires --mixedbread-store or --catalog-uri")?;
        return finish(consume_parquet(&config, mixedbread).await);
    }

    let parquet = match cli.bucket.as_ref() {
        // Host-scope every parquet key. Many fleet hosts index the same account
        // (notably `root`, present on every host) into one shared bucket, and
        // the sink does a full-file overwrite per source; without `host` in the
        // key, host B clobbers host A's `user=root/source=shell/data.parquet`
        // and the per-host manifest skip-gate makes them ping-pong every tick.
        // `host`/`user`/`source` are all hive partitions, matching the old
        // history-ship layout (`host=<host>/user=<user>/...`).
        //
        // Connecting here (once per run, not once per source) also surfaces a
        // misconfigured endpoint or missing credentials at startup instead of
        // as a per-source failure.
        Some(bucket) => {
            let host = resolve_host(&cli).context("resolving host for the parquet archive prefix")?;
            let config = sink_parquet::Config {
                bucket: bucket.clone(),
                endpoint: cli.endpoint.clone(),
                region: cli.region.clone(),
                prefix: archive_prefix(&cli.prefix, &host),
            };
            Some(config.connect().context("building the S3 client for the parquet archive")?)
        }
        None => None,
    };
    // The Iceberg corpus lake (issue #752): the parquet log's successor, run
    // alongside it during the migration. Connecting and ensuring the table here
    // surfaces a bad catalog config at startup, like the parquet connect above.
    let lake = match cli.catalog_uri.as_ref() {
        Some(_) => {
            let Lake { catalog, ident } = connect_lake(&cli).await?;
            let host = resolve_host(&cli).context("resolving host for the lake")?;
            Some(IcebergReconciler::new(catalog, ident, host))
        }
        None => None,
    };
    if store.is_none() && parquet.is_none() && lake.is_none() {
        anyhow::bail!("nothing to do: pass --mixedbread-store, --bucket, and/or --catalog-uri");
    }
    if !any_source_selected(&cli) {
        anyhow::bail!(
            "no sources selected: pass --local, --user NAME:HOME, --claude-dir/--codex-file/--atuin-db/--slack-export/--linear-export/--github-export/--git-repo, or --code-repo"
        );
    }

    finish(run_sources(&cli, mixedbread, parquet.as_ref(), lake.as_ref()).await)
}

/// Build the lake's catalog config from the CLI: the catalog flags plus the
/// shared S3 endpoint/region (the lake's data plane is the same account the
/// parquet archive uses during the migration).
fn lake_config(cli: &Cli) -> anyhow::Result<lake_iceberg::Config> {
    let uri = cli.catalog_uri.clone().context("--catalog-uri is required for the Iceberg lake")?;
    let warehouse = cli.warehouse.clone().context("--warehouse is required with --catalog-uri")?;
    Ok(lake_iceberg::Config {
        uri,
        warehouse,
        token: cli.catalog_token.clone(),
        s3_endpoint: cli.endpoint.clone(),
        s3_region: cli.region.clone(),
    })
}

/// A connected lake: the catalog handle and the corpus table within it.
struct Lake {
    catalog: std::sync::Arc<dyn lake_iceberg::Catalog>,
    ident: lake_iceberg::TableIdent,
}

/// Connect the lake's catalog and ensure its table, shared by the lake sink
/// and every lake consume mode. Failing here surfaces a bad catalog config at
/// startup.
async fn connect_lake(cli: &Cli) -> anyhow::Result<Lake> {
    let config = lake_config(cli)?;
    let catalog = config.connect().await.context("connecting the Iceberg catalog")?;
    let ident =
        lake_iceberg::ensure_table(catalog.as_ref()).await.context("ensuring the lake table")?;
    Ok(Lake { catalog, ident })
}

/// Apply the lake's changes since `cursor` to Mixedbread, returning the
/// snapshot the store is now caught up to (the next cursor).
async fn run_lake_delta<S: Store + Sync>(
    catalog: &dyn lake_iceberg::Catalog,
    ident: &lake_iceberg::TableIdent,
    cursor: i64,
    mixedbread: MixedbreadReconciler<'_, S>,
) -> anyhow::Result<Option<i64>> {
    let delta = lake_iceberg::added_since(catalog, ident, cursor)
        .await
        .context("reading the lake delta")?;
    let to_snapshot = delta.to_snapshot;
    let report = mixedbread
        .apply(delta.upserts, &delta.deletes)
        .await
        .context("applying the lake delta to Mixedbread")?;
    eprintln!(
        "[lake-delta] applied {} upserts, {} deletes (cursor {cursor} -> {to_snapshot:?})",
        report.uploaded, report.deleted
    );
    Ok(to_snapshot)
}

/// Steady-state lake consume: cursor from `path`, delta applied to Mixedbread,
/// new cursor written back. Bootstraps (absent file) and recovers (expired
/// cursor) via a full replace rebuild. The apply is idempotent, so a crash
/// before the cursor write replays safely on the next run.
async fn run_cursor_consume<S: Store + Sync>(
    catalog: &dyn lake_iceberg::Catalog,
    ident: &lake_iceberg::TableIdent,
    path: &Path,
    mixedbread: MixedbreadReconciler<'_, S>,
) -> Counts {
    let mut counts = Counts { indexed: 0, skipped: 0, failures: 0 };
    let cursor = match read_cursor(path) {
        Ok(cursor) => cursor,
        Err(error) => {
            // A malformed cursor file is a real error, not a silent rebuild: it
            // means state corruption worth a human look.
            record("lake-cursor", Err(error), &mut counts);
            return counts;
        }
    };

    if let Some(cursor) = cursor {
        match run_lake_delta(catalog, ident, cursor, mixedbread).await {
            Ok(to_snapshot) => {
                // An empty table has no snapshot to store; keep the old cursor.
                let result = to_snapshot
                    .map_or_else(|| Ok(()), |snapshot| write_cursor(path, snapshot));
                record("lake-cursor", result, &mut counts);
                return counts;
            }
            // The one recoverable failure: snapshot expiration outran the
            // cursor. Fall through to the full rebuild below.
            Err(error)
                if error
                    .downcast_ref::<lake_iceberg::Error>()
                    .is_some_and(|e| matches!(e, lake_iceberg::Error::CursorNotFound { .. })) =>
            {
                eprintln!("[lake-cursor] cursor {cursor} expired; falling back to a full rebuild");
            }
            Err(error) => {
                record("lake-cursor", Err(error), &mut counts);
                return counts;
            }
        }
    }

    // Bootstrap / recovery: full rebuild with replace semantics — the cursor
    // is gone, so tombstones appended while it was lost can never arrive as a
    // delta, and only a full-state diff (deleting view records the lake no
    // longer holds) can apply them before the new cursor buries them. The
    // snapshot is read BEFORE the fold, so appends landing mid-rebuild are
    // replayed next pass rather than skipped.
    let rebuild = async {
        let to_snapshot = lake_iceberg::current_snapshot_id(catalog, ident)
            .await
            .context("reading the lake snapshot")?;
        let state = lake_iceberg::read_state(catalog, ident).await.context("reading the lake")?;
        anyhow::Ok((to_snapshot, state))
    }
    .await;
    match rebuild {
        Ok((to_snapshot, state)) => {
            let replace = run_replace(state, mixedbread).await;
            counts.indexed += replace.indexed;
            counts.skipped += replace.skipped;
            counts.failures += replace.failures;
            if replace.failures == 0
                && let Some(snapshot) = to_snapshot
            {
                record("lake-cursor", write_cursor(path, snapshot), &mut counts);
            }
        }
        Err(error) => record("lake-cursor", Err(error), &mut counts),
    }
    counts
}

/// Read the cursor file: `Ok(None)` when absent (bootstrap), the snapshot id
/// when well-formed, and an error when present but malformed.
fn read_cursor(path: &Path) -> anyhow::Result<Option<i64>> {
    let text = match std::fs::read_to_string(path) {
        Ok(text) => text,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => {
            return Err(anyhow::Error::new(error)
                .context(format!("reading the cursor file {}", path.display())));
        }
    };
    let value: serde_json::Value = serde_json::from_str(&text)
        .with_context(|| format!("parsing the cursor file {}", path.display()))?;
    let snapshot = value
        .get("snapshot")
        .and_then(serde_json::Value::as_i64)
        .with_context(|| format!("cursor file {} has no integer `snapshot`", path.display()))?;
    Ok(Some(snapshot))
}

/// Write the cursor file atomically (temp file + rename, same directory), so a
/// crash mid-write can never leave a truncated cursor.
fn write_cursor(path: &Path, snapshot: i64) -> anyhow::Result<()> {
    let tmp = path.with_extension("json.tmp");
    let body = serde_json::json!({ "snapshot": snapshot }).to_string();
    std::fs::write(&tmp, body)
        .with_context(|| format!("writing the cursor temp file {}", tmp.display()))?;
    std::fs::rename(&tmp, path)
        .with_context(|| format!("renaming the cursor file into place at {}", path.display()))?;
    Ok(())
}

/// Turn the per-run counts into the process result: success only when no source
/// failed, so a partial failure is a non-zero exit the timer/operator can see.
///
/// Soft skips (e.g. an uninitialized atuin db, counted in `skipped`) never gate
/// the exit code — only genuine `failures` do — so one account whose history db
/// has no `history` table cannot degrade the whole indexing unit.
fn finish(counts: Counts) -> anyhow::Result<()> {
    if counts.skipped > 0 {
        eprintln!("[indexer] {} source(s) soft-skipped (uninitialized/empty)", counts.skipped);
    }
    if counts.failures > 0 {
        anyhow::bail!(
            "{} of {} source(s) failed; {} succeeded, {} skipped",
            counts.failures,
            counts.indexed + counts.failures + counts.skipped,
            counts.indexed,
            counts.skipped
        );
    }
    Ok(())
}

/// Whether any source flag was given (a config check, independent of how many
/// records each source ends up producing).
const fn any_source_selected(cli: &Cli) -> bool {
    cli.local
        || cli.claude_dir.is_some()
        || cli.codex_file.is_some()
        || cli.atuin_db.is_some()
        || cli.slack_export.is_some()
        || cli.linear_export.is_some()
        || cli.github_export.is_some()
        || !cli.git_repos.is_empty()
        || !cli.code_repos.is_empty()
        || !cli.users.is_empty()
}

/// Resolve the selected sources and run each one independently (a failure never
/// aborts the others), returning `(indexed, failed)` counts.
async fn run_sources(
    cli: &Cli,
    mixedbread: Option<Mixedbread<'_>>,
    parquet: Option<&ParquetReconciler>,
    lake: Option<&IcebergReconciler>,
) -> Counts {
    let home = dirs::home_dir();
    let default = |suffix: &str| home.as_ref().map(|h| h.join(suffix));
    let claude = cli.claude_dir.clone().or_else(|| cli.local.then(|| default(".claude/projects")).flatten());
    let codex = cli.codex_file.clone().or_else(|| cli.local.then(|| default(".codex/history.jsonl")).flatten());
    let atuin = cli.atuin_db.clone().or_else(|| cli.local.then(|| default(".local/share/atuin/history.db")).flatten());

    let mut counts = Counts { indexed: 0, skipped: 0, failures: 0 };
    if let Some(dir) = claude {
        let result = async {
            let adapter = source_claude::ClaudeHistoryExport::open(&dir)
                .with_context(|| format!("parsing Claude transcripts at {}", dir.display()))?;
            run_source("claude", &adapter, mixedbread, parquet, lake).await
        }
        .await;
        record("claude", result, &mut counts);
    }
    if let Some(file) = codex {
        let result = async {
            let adapter = source_codex::CodexHistory::open(&file)
                .with_context(|| format!("parsing Codex history at {}", file.display()))?;
            run_source("codex", &adapter, mixedbread, parquet, lake).await
        }
        .await;
        record("codex", result, &mut counts);
    }
    if let Some(db) = atuin {
        match open_atuin(
            "shell",
            &db,
            mixedbread.is_some() || parquet.is_some() || lake.is_some(),
            &mut counts,
        ) {
            Ok(Atuin::Ready(adapter)) => {
                let result = run_source("shell", &adapter, mixedbread, parquet, lake).await;
                record("shell", result, &mut counts);
            }
            // An uninitialized db is already logged and tallied as a soft skip.
            Ok(Atuin::Skipped) => {}
            Err(error) => record("shell", Err(error), &mut counts),
        }
    }
    if let Some(dir) = &cli.slack_export {
        let result = async {
            let adapter = source_slack::SlackExport::open(dir)
                .with_context(|| format!("reading Slack export at {}", dir.display()))?;
            run_source("slack", &adapter, mixedbread, parquet, lake).await
        }
        .await;
        record("slack", result, &mut counts);
    }
    if let Some(dir) = &cli.linear_export {
        let result = async {
            let adapter = source_linear::LinearExport::open(dir)
                .with_context(|| format!("reading Linear export at {}", dir.display()))?;
            run_source("linear", &adapter, mixedbread, parquet, lake).await
        }
        .await;
        record("linear", result, &mut counts);
    }
    if let Some(dir) = &cli.github_export {
        let result = async {
            let adapter = source_github::GithubExport::open(dir)
                .with_context(|| format!("reading GitHub export at {}", dir.display()))?;
            run_source("github", &adapter, mixedbread, parquet, lake).await
        }
        .await;
        record("github", result, &mut counts);
    }
    for repo in &cli.git_repos {
        let label = format!("git:{}", repo.display());
        let result = async {
            let adapter = source_git::GitLog::open(repo)
                .with_context(|| format!("reading git history at {}", repo.display()))?;
            run_source("git", &adapter, mixedbread, parquet, lake).await
        }
        .await;
        record(&label, result, &mut counts);
    }
    for repo_dir in &cli.code_repos {
        let label = format!("code:{}", repo_dir.display());
        let result = index_code(&label, repo_dir, mixedbread).await;
        record(&label, result, &mut counts);
    }
    if !cli.users.is_empty() {
        run_users(cli, mixedbread, parquet, lake, &mut counts).await;
    }
    counts
}

/// Run the `--user NAME:HOME` multi-user phase, accumulating into the shared
/// counters. Split out of [`run_sources`] to keep each function focused.
async fn run_users(
    cli: &Cli,
    mixedbread: Option<Mixedbread<'_>>,
    parquet: Option<&ParquetReconciler>,
    lake: Option<&IcebergReconciler>,
    counts: &mut Counts,
) {
    let host = match resolve_host(cli) {
        Ok(host) => host,
        Err(error) => {
            // Without a host tag every claude/codex record would be mislabeled,
            // so fail the whole multi-user phase rather than emit wrong metadata.
            // Count it as one phase failure (no per-user work ran).
            eprintln!("[users] failed to resolve host, skipping all --user sources: {error:#}");
            counts.failures += 1;
            return;
        }
    };
    for spec in &cli.users {
        match parse_user(spec) {
            Ok(user) => index_user(&user, &host, mixedbread, parquet, lake, counts).await,
            Err(error) => {
                eprintln!("[users] bad --user spec: {error:#}");
                counts.failures += 1;
            }
        }
    }
}

/// Consume the parquet corpus log: read the per-source `data.parquet` files into
/// documents, then reconcile them into Mixedbread via [`run_consume`].
async fn consume_parquet(config: &source_parquet::Config, mixedbread: Mixedbread<'_>) -> Counts {
    let documents = match source_parquet::read_documents(config).await {
        Ok(documents) => documents,
        Err(error) => {
            eprintln!("[consume] failed to read the parquet corpus log: {error:#}");
            return Counts { indexed: 0, skipped: 0, failures: 1 };
        }
    };
    run_consume(documents, mixedbread).await
}

/// Fold the parquet corpus archive into the Iceberg lake, one slice per
/// `(host, user, source)`: the leader's parquet->lake step under leader-funnel.
///
/// Each slice reconciles scoped to its origin host and user, so a shared
/// `external_id` on two hosts stays two rows instead of one host silently
/// clobbering the other (issue #752). A slice failure is logged and tallied but
/// never aborts the rest, matching [`run_source`].
async fn fold_parquet_into_lake(
    config: &source_parquet::Config,
    catalog: std::sync::Arc<dyn lake_iceberg::Catalog>,
    ident: &lake_iceberg::TableIdent,
) -> Counts {
    let slices = match source_parquet::read_slices(config).await {
        Ok(slices) => slices,
        Err(error) => {
            eprintln!("[fold] failed to read the parquet corpus log: {error:#}");
            return Counts { indexed: 0, skipped: 0, failures: 1 };
        }
    };
    let mut counts = Counts { indexed: 0, skipped: 0, failures: 0 };
    for slice in slices {
        let source_parquet::Slice { host, user, source, documents } = slice;
        let Some(host) = host else {
            eprintln!("[fold:{source}] skipping a slice whose key has no host= segment");
            counts.failures += 1;
            continue;
        };
        let mut reconciler =
            IcebergReconciler::new(std::sync::Arc::clone(&catalog), ident.clone(), host);
        if let Some(user) = user {
            reconciler = reconciler.with_user(user);
        }
        let source = Source::new(source);
        match reconciler.reconcile(&source, &documents).await {
            Ok(report) if report.skipped => {
                eprintln!("[fold:{}] skipped (unchanged)", source.as_str());
                counts.skipped += 1;
            }
            Ok(report) => {
                eprintln!(
                    "[fold:{}] appended {} upserts, {} tombstones",
                    source.as_str(),
                    report.upserts,
                    report.deletes
                );
                counts.indexed += 1;
            }
            Err(error) => {
                eprintln!("[fold:{}] failed: {error:#}", source.as_str());
                counts.failures += 1;
            }
        }
    }
    counts
}

/// Replace each lake source's Mixedbread records with the lake's live fold:
/// upload the new or changed, delete the records the lake no longer holds.
///
/// Only sources present in the lake are touched, so a store shared with
/// directly indexed sources (code repos) keeps those intact — and a source
/// whose lake records are all tombstoned still gets its view records deleted.
/// The lake consume paths use this instead of [`run_consume`] because the
/// lake's absences are explicit tombstone folds, while the parquet log has no
/// tombstones and its absences stay protective.
async fn run_replace<S: Store + Sync>(
    state: lake_iceberg::LakeState,
    mixedbread: MixedbreadReconciler<'_, S>,
) -> Counts {
    let mut counts = Counts { indexed: 0, skipped: 0, failures: 0 };
    for (source, documents) in state.sources {
        let label = format!("replace:{source}");
        let result = mixedbread
            .replace(&Source::new(source), &documents)
            .await
            .map(|report| {
                eprintln!(
                    "[{label}] mixedbread: uploaded {}, skipped {}, deleted {} of {}",
                    report.uploaded, report.skipped, report.deleted, report.total
                );
            })
            .with_context(|| format!("[{label}] Mixedbread replace"));
        record(&label, result, &mut counts);
    }
    counts
}

/// Reconcile already-read documents into Mixedbread, grouped by their `source`.
///
/// Grouping keeps each Mixedbread reconcile scoped to one source, exactly as the
/// direct per-source ingestion did, so a consumed record dedups against its own
/// source and never touches another's. The parquet consume path reads its
/// documents first, then shares this reconcile (the lake paths replace instead;
/// see [`run_replace`]).
async fn run_consume(documents: Vec<Document>, mixedbread: Mixedbread<'_>) -> Counts {
    let mut counts = Counts { indexed: 0, skipped: 0, failures: 0 };
    let mut by_source: BTreeMap<String, Vec<Document>> = BTreeMap::new();
    for document in documents {
        let source = document
            .meta_json
            .get(keys::SOURCE)
            .and_then(serde_json::Value::as_str)
            .unwrap_or("unknown")
            .to_owned();
        by_source.entry(source).or_default().push(document);
    }

    for (source, documents) in by_source {
        let label = format!("consume:{source}");
        let result = mixedbread
            .reconcile(&Source::new(source), &documents)
            .await
            .map(|report| {
                eprintln!(
                    "[{label}] mixedbread: uploaded {}, skipped {} of {}",
                    report.uploaded, report.skipped, report.total
                );
            })
            .with_context(|| format!("[{label}] Mixedbread reconcile"));
        record(&label, result, &mut counts);
    }
    counts
}

/// Index one user's local agent and shell history (claude, codex, atuin),
/// reading under `home` and tagging records with `name` and `host`.
///
/// Designed for the privileged fleet run. Every history path is resolved with
/// [`safe_path_under`], which refuses to follow a symlink at any user-controlled
/// component, so a planted symlink cannot redirect a root read at another
/// account's files (the claude adapter additionally refuses symlinks inside its
/// tree). Absent sources are skipped; a parse failure in one source is recorded
/// but does not abort the others.
async fn index_user(
    user: &User,
    host: &str,
    mixedbread: Option<Mixedbread<'_>>,
    parquet: Option<&ParquetReconciler>,
    lake: Option<&IcebergReconciler>,
    counts: &mut Counts,
) {
    let name = user.name.as_str();
    let home = user.home.as_path();
    // User-scope the parquet log so several accounts on one host do not clobber
    // each other's `source=<source>/data.parquet` (the sink overwrites that file
    // in full per run, and every account produces the same `source=claude` etc.).
    // The lake scopes the same way: its slice (and so its tombstones) must be
    // per-account, or one user's reconcile would delete another's documents.
    // The Mixedbread sink needs no such scoping: its `external_id`s already carry
    // the per-message uuid, so records never collide across users there.
    let user_parquet =
        parquet.map(|reconciler| reconciler.with_prefix(user_prefix(&reconciler.prefix, name)));
    let parquet = user_parquet.as_ref();
    let user_lake = lake.map(|reconciler| reconciler.with_user(name));
    let lake = user_lake.as_ref();
    if let Some(claude_dir) = safe_path_under(home, &[".claude", "projects"], true) {
        let label = format!("claude:{name}");
        let result = async {
            let adapter = source_claude::ClaudeHistoryExport::open_with(&claude_dir, host, name)
                .with_context(|| format!("parsing Claude transcripts for {name} at {}", claude_dir.display()))?;
            run_source(&label, &adapter, mixedbread, parquet, lake).await
        }
        .await;
        record(&label, result, counts);
    }

    if let Some(codex_file) = safe_path_under(home, &[".codex", "history.jsonl"], false) {
        let label = format!("codex:{name}");
        let result = async {
            let adapter = source_codex::CodexHistory::open_with(&codex_file, host, name)
                .with_context(|| format!("parsing Codex history for {name} at {}", codex_file.display()))?;
            run_source(&label, &adapter, mixedbread, parquet, lake).await
        }
        .await;
        record(&label, result, counts);
    }

    // atuin records its own `host`/`user` in each row, so it self-tags per user
    // regardless of who runs the process. An account whose db file exists but
    // was never initialized by atuin (no `history` table) is a soft skip, so one
    // such account cannot fail the whole fleet run (ENG-2141).
    if let Some(atuin_db) = safe_path_under(home, &[".local", "share", "atuin", "history.db"], false) {
        let label = format!("shell:{name}");
        match open_atuin(
            &label,
            &atuin_db,
            mixedbread.is_some() || parquet.is_some() || lake.is_some(),
            counts,
        ) {
            Ok(Atuin::Ready(adapter)) => {
                let result = run_source(&label, &adapter, mixedbread, parquet, lake).await;
                record(&label, result, counts);
            }
            Ok(Atuin::Skipped) => {}
            Err(error) => record(&label, Err(error), counts),
        }
    }

    // Claude debug logs (`~/.claude/debug/<session>.txt`), present only for
    // sessions run with `--debug`. The adapter indexes regular files only, so a
    // planted symlink in the debug dir is skipped rather than followed.
    if let Some(debug_dir) = safe_path_under(home, &[".claude", "debug"], true) {
        let label = format!("debug:{name}");
        let result = async {
            let adapter = source_debug::DebugLogs::open_with(&debug_dir, host, name)
                .with_context(|| format!("reading Claude debug logs for {name} at {}", debug_dir.display()))?;
            run_source(&label, &adapter, mixedbread, parquet, lake).await
        }
        .await;
        record(&label, result, counts);
    }
}

/// Parse a `NAME:HOME` user spec. The name is everything before the first colon;
/// both parts must be non-empty.
fn parse_user(spec: &str) -> anyhow::Result<User> {
    let (name, home) =
        spec.split_once(':').with_context(|| format!("--user must be NAME:HOME, got {spec:?}"))?;
    anyhow::ensure!(!name.is_empty(), "--user NAME must be non-empty in {spec:?}");
    anyhow::ensure!(!home.is_empty(), "--user HOME must be non-empty in {spec:?}");
    // NAME becomes a metadata tag and a `user=<name>` parquet partition segment,
    // so keep it to a safe charset (no `/` or `=` that could cross partitions).
    anyhow::ensure!(
        name.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-')),
        "--user NAME must be ascii alphanumeric plus `.`/`_`/`-`, got {name:?}"
    );
    Ok(User { name: name.to_owned(), home: PathBuf::from(home) })
}

/// Host-scope the parquet archive prefix: `<base>/host=<host>`. Every fleet host
/// writes the same shared accounts (notably `root`) into one bucket with a
/// full-file overwrite per source, so without a `host` segment they clobber each
/// other and ping-pong on the per-host manifest skip-gate. `host` is the
/// outermost hive partition, matching the old history-ship layout.
fn archive_prefix(base: &str, host: &str) -> String {
    format!("{base}/host={host}")
}

/// User-scope a parquet prefix for the multi-user `--user` path:
/// `<host-prefix>/user=<name>`. `sink-parquet` writes one full-file-overwrite
/// `source=<source>/data.parquet` per (prefix, source), and several accounts on
/// one host produce the same `source=claude` (etc.), so without a `user` segment
/// each account would clobber the last in the durable log. `name` is validated by
/// [`parse_user`] to a safe charset (no `/`/`=`), so it cannot escape the
/// partition. `user` is the inner hive partition under `host`, matching the old
/// history-ship `host=<host>/user=<user>/...` layout.
fn user_prefix(base: &str, name: &str) -> String {
    format!("{base}/user={name}")
}

/// Resolve a user-controlled subpath under a trusted `home`, refusing to follow
/// a symlink at any component. Returns the joined path only if every component in
/// `rel` is a real (non-symlink) entry — intermediate ones directories, and the
/// final one matching `want_dir` (a directory) or `!want_dir` (a regular file).
///
/// `home` comes from the system user database and is trusted; everything under
/// it is attacker-controlled when this runs as root over other accounts. lstat'ing
/// every component (not just the leaf) is what stops a planted symlink — at the
/// root, an ancestor, or the leaf — from redirecting the read at another
/// account's files (the confused-deputy class; see ix `history-ship`).
///
/// A narrow TOCTOU remains: a component could be swapped for a symlink between
/// this check and the adapter's open. That residual race matches `history-ship`'s
/// posture and is tracked for a shared `openat2(RESOLVE_NO_SYMLINKS)` hardening
/// across both readers.
fn safe_path_under(home: &Path, rel: &[&str], want_dir: bool) -> Option<PathBuf> {
    let last = rel.len().checked_sub(1)?;
    let mut path = home.to_path_buf();
    for (index, component) in rel.iter().enumerate() {
        path.push(component);
        let meta = std::fs::symlink_metadata(&path).ok()?;
        if meta.file_type().is_symlink() {
            return None;
        }
        let ok = if index == last {
            if want_dir { meta.is_dir() } else { meta.is_file() }
        } else {
            meta.is_dir()
        };
        if !ok {
            return None;
        }
    }
    Some(path)
}

/// The host name to tag `--user` records with: the `--host` override, else the
/// system hostname.
fn resolve_host(cli: &Cli) -> anyhow::Result<String> {
    if let Some(host) = &cli.host {
        return Ok(host.clone());
    }
    let raw = nix::unistd::gethostname().context("resolving the system host name")?;
    Ok(raw.to_string_lossy().into_owned())
}

/// Index one code checkout via search-core's content-addressed reconcile
/// (Mixedbread only — code lives in git, not the parquet archive). Reuses the
/// exact code sync a bare `search` would run, so records are byte-identical
/// (same hashes, same repo scoping).
async fn index_code(
    label: &str,
    repo_dir: &std::path::Path,
    mixedbread: Option<Mixedbread<'_>>,
) -> anyhow::Result<()> {
    let Some(Mixedbread { store, name, .. }) = mixedbread else {
        anyhow::bail!("--code-repo requires --mixedbread-store (code is semantic-search only)");
    };
    let manifest = search_core::Manifest::build(repo_dir, None, MAX_FILE_BYTES)
        .with_context(|| format!("building manifest for {}", repo_dir.display()))?;
    let repo = search_core::repo_slug(repo_dir);
    let report = search_core::sync(store, name, repo_dir, &manifest, &repo, MAX_FILES, |_, _| {})
        .await
        .with_context(|| format!("[{label}] code sync"))?;
    if report.uploaded > 0 {
        search_core::wait_until_indexed(store, name, INDEX_TIMEOUT, |_| {})
            .await
            .with_context(|| format!("[{label}] waiting for indexing"))?;
    }
    eprintln!(
        "[{label}] mixedbread: uploaded {}, skipped {} of {}",
        report.uploaded, report.skipped, report.total
    );
    Ok(())
}

/// Record one source's outcome. A failure is logged and counted but does not
/// abort the run, so one broken source cannot starve the others.
fn record(label: &str, result: anyhow::Result<()>, counts: &mut Counts) {
    match result {
        Ok(()) => counts.indexed += 1,
        Err(error) => {
            eprintln!("[{label}] failed: {error:#}");
            counts.failures += 1;
        }
    }
}

/// The outcome of opening one atuin db: a parsed source to index, or a logged,
/// non-fatal skip already tallied into [`Counts::skipped`].
enum Atuin {
    /// The db opened and its `history` table was read.
    Ready(source_atuin::AtuinHistory),
    /// The db was uninitialized (no `history` table) and is being skipped.
    Skipped,
}

/// Open one atuin history db, folding the "uninitialized db" case into a logged
/// soft skip rather than a hard error.
///
/// The fleet run reads every account's history; an account that has an atuin db
/// file but never ran atuin has a db with no `history` table. That is a benign,
/// expected state — not a read failure — so it is recorded in
/// [`Counts::skipped`] and never gates the unit's exit code. Any other open
/// failure (a corrupt db, a permissions error) is still returned for the caller
/// to record as a genuine failure, preserving real-error reporting.
fn open_atuin(
    label: &str,
    db: &Path,
    has_sink: bool,
    counts: &mut Counts,
) -> anyhow::Result<Atuin> {
    match source_atuin::AtuinHistory::open(db) {
        Ok(history) => Ok(Atuin::Ready(history)),
        Err(error) if error.is_uninitialized() => {
            // A run that selects a source but configures no sink is a
            // misconfiguration. run_source rejects it for a readable db; enforce
            // the same here BEFORE downgrading to a soft skip, so an uninitialized
            // db cannot let a sink-less run exit 0 when the identical config fails
            // once the `history` table exists.
            anyhow::ensure!(
                has_sink,
                "[{label}] no sink configured: pass --mixedbread-store and/or --bucket"
            );
            eprintln!("[{label}] skipped: {error} ({db})", db = db.display());
            counts.skipped += 1;
            Ok(Atuin::Skipped)
        }
        Err(error) => Err(anyhow::Error::new(error)
            .context(format!("reading atuin history at {}", db.display()))),
    }
}

/// Fan one source out to every enabled sink.
///
/// The durable logs run FIRST (parquet, then the Iceberg lake) and every sink
/// is INDEPENDENT: a slow or failing Mixedbread upload must not gate or skip a
/// durable write. The log writes are fast object-store puts, while the
/// Mixedbread leg is network-bound and rate-limited (429 + backoff), so
/// ordering it last lands the queryable logs in seconds instead of after a
/// multi-hour upload. Each sink's error is captured separately and only
/// combined at the end, so one sink's failure still lets the others run.
async fn run_source<A: SourceAdapter + Sync>(
    label: &str,
    adapter: &A,
    mixedbread: Option<Mixedbread<'_>>,
    parquet: Option<&ParquetReconciler>,
    lake: Option<&IcebergReconciler>,
) -> anyhow::Result<()> {
    // A selected source with no sink is a misconfiguration, not a no-op: a missing
    // `--mixedbread-store`/`--bucket`/`--catalog-uri` would otherwise drop the
    // source silently while still counting as a success.
    anyhow::ensure!(
        mixedbread.is_some() || parquet.is_some() || lake.is_some(),
        "[{label}] no sink configured: pass --mixedbread-store, --bucket, and/or --catalog-uri"
    );

    // One pass over the adapter feeds every view (each sink used to re-run, and
    // re-parse, the source's iterator independently). An adapter error fails the
    // source before either view is touched, exactly as it failed both sinks
    // mid-iteration before.
    let source = adapter.source();
    let documents = adapter
        .documents()
        .collect::<Result<Vec<Document>, _>>()
        .with_context(|| format!("[{label}] reading documents"))?;

    let mut errors: Vec<anyhow::Error> = Vec::new();

    if let Some(reconciler) = parquet {
        match reconciler.reconcile(&source, &documents).await {
            Ok(report) if report.skipped => eprintln!("[{label}] parquet: skipped (unchanged)"),
            Ok(report) => eprintln!("[{label}] parquet: wrote {} rows", report.rows),
            Err(error) => {
                errors.push(anyhow::Error::new(error).context(format!("[{label}] parquet sync")));
            }
        }
    }

    if let Some(reconciler) = lake {
        match reconciler.reconcile(&source, &documents).await {
            Ok(report) if report.skipped => eprintln!("[{label}] lake: skipped (unchanged)"),
            Ok(report) => eprintln!(
                "[{label}] lake: appended {} upserts, {} tombstones",
                report.upserts, report.deletes
            ),
            Err(error) => {
                errors.push(anyhow::Error::new(error).context(format!("[{label}] lake sync")));
            }
        }
    }

    if let Some(reconciler) = mixedbread {
        match reconciler.reconcile(&source, &documents).await {
            Ok(report) => eprintln!(
                "[{label}] mixedbread: uploaded {}, skipped {} of {}",
                report.uploaded, report.skipped, report.total
            ),
            Err(error) => {
                errors.push(anyhow::Error::new(error).context(format!("[{label}] Mixedbread sync")));
            }
        }
    }

    // Surface every sink failure; a single combined error keeps the per-source
    // failure accounting in `record` intact while not hiding the second sink.
    match errors.len() {
        0 => Ok(()),
        1 => Err(errors.into_iter().next().expect("len checked")),
        _ => {
            let combined =
                errors.iter().map(|error| format!("{error:#}")).collect::<Vec<_>>().join("; ");
            Err(anyhow::anyhow!("[{label}] multiple sinks failed: {combined}"))
        }
    }
}

#[cfg(test)]
mod tests {
    #![expect(clippy::expect_used, reason = "tests assert observable filesystem outcomes")]

    use std::collections::HashMap;
    use std::path::PathBuf;
    use std::sync::Arc;
    use std::time::Duration;

    use iceberg::CatalogBuilder as _;
    use lake_iceberg::IcebergReconciler;
    use search_core::{MemoryStore, Store as _};
    use sink_mixedbread::MixedbreadReconciler;
    use source_meta::{Reconciler as _, Source};

    use super::{
        Atuin, Counts, archive_prefix, finish, open_atuin, parse_user, read_cursor,
        run_cursor_consume, safe_path_under, user_prefix, write_cursor,
    };

    /// Create a valid sqlite db with no `history` table at `path`, mirroring
    /// atuin's pre-first-run state (the file exists before migrations add tables).
    fn make_uninitialized_db(path: &std::path::Path) {
        rusqlite::Connection::open(path).expect("create empty sqlite db");
    }

    #[test]
    fn uninitialized_atuin_db_is_soft_skipped_and_run_succeeds() {
        // ENG-2141: one account whose atuin db exists but has no `history` table
        // (atuin never ran there) must be a logged soft skip, not a failure, so
        // the whole indexing unit still succeeds. `has_sink` is true: a sink is
        // configured, it just never gets written in this test.
        let temp = tempfile::tempdir().expect("tempdir");
        let db = temp.path().join("history.db");
        make_uninitialized_db(&db);

        let mut counts = Counts { indexed: 0, skipped: 0, failures: 0 };
        let outcome = open_atuin("shell:tester", &db, true, &mut counts)
            .expect("uninitialized db is not an error");

        assert!(matches!(outcome, Atuin::Skipped), "uninitialized db must be skipped");
        assert_eq!(counts.skipped, 1, "the skip must be tallied");
        assert_eq!(counts.failures, 0, "an uninitialized db must not count as a failure");
        // The run as a whole still succeeds: no failures means a zero exit.
        assert!(finish(counts).is_ok(), "a soft-skipped source must not fail the run");
    }

    #[test]
    fn missing_atuin_db_is_a_real_error() {
        // A genuinely missing file (nothing to open) is a hard error so real
        // failures are still surfaced; only the uninitialized-db case is a skip.
        let temp = tempfile::tempdir().expect("tempdir");
        let db = temp.path().join("does-not-exist.db");

        let mut counts = Counts { indexed: 0, skipped: 0, failures: 0 };
        assert!(
            open_atuin("shell:tester", &db, true, &mut counts).is_err(),
            "a missing db file must remain a real error, not a soft skip"
        );
        assert_eq!(counts.skipped, 0, "a real error must not be tallied as a skip");
    }

    #[test]
    fn uninitialized_atuin_db_without_sink_is_an_error() {
        // The soft skip must not bypass sink validation: an uninitialized db with
        // no sink is the same misconfiguration run_source rejects once the db has a
        // `history` table, so it must fail consistently rather than exit 0 (the
        // per-user fleet path shares open_atuin, so it is covered too).
        let temp = tempfile::tempdir().expect("tempdir");
        let db = temp.path().join("history.db");
        make_uninitialized_db(&db);

        let mut counts = Counts { indexed: 0, skipped: 0, failures: 0 };
        assert!(
            open_atuin("shell:tester", &db, false, &mut counts).is_err(),
            "an uninitialized db with no sink must error, not silently skip"
        );
        assert_eq!(counts.skipped, 0, "a misconfiguration must not be tallied as a skip");
    }

    #[test]
    fn safe_path_accepts_real_nested_dir() {
        let temp = tempfile::tempdir().expect("tempdir");
        std::fs::create_dir_all(temp.path().join(".claude").join("projects")).expect("mkdir");
        assert!(safe_path_under(temp.path(), &[".claude", "projects"], true).is_some());
    }

    #[test]
    fn safe_path_rejects_symlinked_leaf() {
        let temp = tempfile::tempdir().expect("tempdir");
        let home = temp.path();
        std::fs::create_dir_all(home.join(".codex")).expect("mkdir");
        let secret = home.join("secret");
        std::fs::write(&secret, b"x").expect("write");
        std::os::unix::fs::symlink(&secret, home.join(".codex").join("history.jsonl")).expect("symlink");
        assert!(safe_path_under(home, &[".codex", "history.jsonl"], false).is_none());
    }

    #[test]
    fn safe_path_rejects_symlinked_ancestor() {
        // The privileged threat: a user points an ancestor component at another
        // tree so the root process reads through it.
        let temp = tempfile::tempdir().expect("tempdir");
        let home = temp.path();
        let victim = home.join("victim");
        std::fs::create_dir_all(victim.join("projects")).expect("mkdir");
        std::fs::write(victim.join("projects").join("s.jsonl"), b"{}").expect("write");
        std::os::unix::fs::symlink(&victim, home.join(".claude")).expect("symlink");
        assert!(
            safe_path_under(home, &[".claude", "projects"], true).is_none(),
            "a symlinked ancestor component must be rejected"
        );
    }

    #[test]
    fn safe_path_missing_is_none() {
        let temp = tempfile::tempdir().expect("tempdir");
        assert!(safe_path_under(temp.path(), &[".codex", "history.jsonl"], false).is_none());
    }

    #[test]
    fn safe_path_rejects_wrong_kind() {
        let temp = tempfile::tempdir().expect("tempdir");
        std::fs::create_dir_all(temp.path().join(".claude").join("projects")).expect("mkdir");
        // `projects` is a directory, but a file was requested.
        assert!(safe_path_under(temp.path(), &[".claude", "projects"], false).is_none());
    }

    #[test]
    fn parse_user_validates_name_and_spec() {
        assert!(parse_user("a/b:/home/x").is_err(), "slash in name");
        assert!(parse_user("a=b:/home/x").is_err(), "equals in name");
        assert!(parse_user(":/home/x").is_err(), "empty name");
        assert!(parse_user("alice:").is_err(), "empty home");
        assert!(parse_user("noseparator").is_err(), "missing colon");
        let user = parse_user("alice-1.2_3:/home/alice").expect("valid spec");
        assert_eq!(user.name, "alice-1.2_3");
        assert_eq!(user.home, PathBuf::from("/home/alice"));
    }

    #[test]
    fn archive_prefix_is_host_scoped() {
        // Bulk exports still host-scope their parquet keys, so two hosts writing
        // the same bucket never clobber each other.
        assert_eq!(archive_prefix("corpus", "hil-compute-1"), "corpus/host=hil-compute-1");
        assert_ne!(archive_prefix("corpus", "a"), archive_prefix("corpus", "b"));
    }

    #[test]
    fn cursor_file_bootstraps_then_round_trips() {
        let temp = tempfile::tempdir().expect("tempdir");
        let path = temp.path().join("cursor.json");
        // Absent file is the bootstrap signal, not an error.
        assert_eq!(read_cursor(&path).expect("absent file"), None);
        write_cursor(&path, 42).expect("write");
        assert_eq!(read_cursor(&path).expect("read back"), Some(42));
        write_cursor(&path, 43).expect("overwrite");
        assert_eq!(read_cursor(&path).expect("read back"), Some(43));
        assert!(!path.with_extension("json.tmp").exists(), "the temp file must not linger");
    }

    #[test]
    fn malformed_cursor_file_is_an_error_not_a_silent_rebuild() {
        let temp = tempfile::tempdir().expect("tempdir");
        let path = temp.path().join("cursor.json");
        std::fs::write(&path, "not json").expect("write garbage");
        assert!(read_cursor(&path).is_err(), "garbage must surface, not bootstrap");
        std::fs::write(&path, "{}").expect("write empty object");
        assert!(read_cursor(&path).is_err(), "a missing `snapshot` key must surface");
    }

    #[test]
    fn user_prefix_adds_user_partition() {
        // Several accounts on one host produce the same `source=claude`, so the
        // per-user parquet log must add a `user=` segment under the host prefix or
        // they clobber each other in the full-file-overwrite sink.
        let base = archive_prefix("corpus", "hil-compute-1");
        assert_eq!(user_prefix(&base, "alice"), "corpus/host=hil-compute-1/user=alice");
        assert_ne!(
            user_prefix(&base, "alice"),
            user_prefix(&base, "bob"),
            "two users must not share a parquet prefix"
        );
    }

    /// A `source=test` document for the lake-consume tests.
    fn lake_doc(id: &str) -> source_meta::Document {
        let body = format!("body of {id}");
        let content_hash = source_meta::hash_body(body.as_bytes());
        source_meta::Document {
            external_id: id.to_owned(),
            file_name: id.to_owned(),
            mime: "text/plain",
            body: body.into_bytes(),
            meta_json: serde_json::json!({
                "source": "test",
                "external_id": id,
                "content_hash": content_hash,
            }),
            content_hash,
        }
    }

    /// The store's current external ids, for asserting view state.
    async fn stored_ids(store: &MemoryStore) -> Vec<String> {
        let mut ids: Vec<String> =
            store.list_external_ids("s", None).await.expect("list").into_iter().collect();
        ids.sort();
        ids
    }

    #[tokio::test]
    async fn cursor_rebuild_gcs_tombstones_missed_while_the_cursor_was_gone() {
        // The recovery contract: when the cursor is absent or expired, the
        // rebuild must not just re-upload the lake's current documents — it
        // must also DELETE view records whose lake rows were tombstoned while
        // no consumer was watching, because writing the new cursor buries
        // those tombstones forever.
        let dir = tempfile::tempdir().expect("tempdir");
        let warehouse = format!("file://{}", dir.path().display());
        let catalog = iceberg::memory::MemoryCatalogBuilder::default()
            .load(
                "lake",
                HashMap::from([(iceberg::memory::MEMORY_CATALOG_WAREHOUSE.to_owned(), warehouse)]),
            )
            .await
            .expect("memory catalog");
        let catalog: Arc<dyn lake_iceberg::Catalog> = Arc::new(catalog);
        let ident = lake_iceberg::ensure_table(catalog.as_ref()).await.expect("ensure table");
        let sink = IcebergReconciler::new(Arc::clone(&catalog), ident.clone(), "host-1");
        let source = Source::new("test");
        let store = MemoryStore::new();
        let mixedbread =
            MixedbreadReconciler { store: &store, name: "s", index_timeout: Duration::from_secs(1) };
        let cursor = dir.path().join("cursor.json");

        // Bootstrap (absent cursor file): a full rebuild lands both documents.
        sink.reconcile(&source, &[lake_doc("a"), lake_doc("b")]).await.expect("seed");
        let counts = run_cursor_consume(catalog.as_ref(), &ident, &cursor, mixedbread).await;
        assert_eq!(counts.failures, 0);
        assert_eq!(stored_ids(&store).await, ["a", "b"]);
        read_cursor(&cursor).expect("cursor readable").expect("cursor written");

        // While no consumer watches, `a` is tombstoned — then the cursor
        // expires (an unknown snapshot id is exactly what expiry surfaces as).
        sink.reconcile(&source, &[lake_doc("b")]).await.expect("tombstone a");
        write_cursor(&cursor, 0).expect("plant an expired cursor");
        let counts = run_cursor_consume(catalog.as_ref(), &ident, &cursor, mixedbread).await;
        assert_eq!(counts.failures, 0);
        assert_eq!(
            stored_ids(&store).await,
            ["b"],
            "the rebuild must GC the record tombstoned while the cursor was gone"
        );
        let rebuilt = read_cursor(&cursor).expect("cursor readable").expect("cursor rewritten");
        assert_ne!(rebuilt, 0, "the rebuild must store the snapshot it read");

        // Steady state after the rebuild: the next change arrives as a delta.
        sink.reconcile(&source, &[lake_doc("c")]).await.expect("replace b with c");
        let counts = run_cursor_consume(catalog.as_ref(), &ident, &cursor, mixedbread).await;
        assert_eq!(counts.failures, 0);
        assert_eq!(stored_ids(&store).await, ["c"], "the delta applies b's tombstone and c");
    }
}
