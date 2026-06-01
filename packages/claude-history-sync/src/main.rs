//! `claude-history-sync`: sync Claude Code transcripts to R2 parquet (queryable
//! with polars) and Mixedbread (semantic search). One parse pass feeds both
//! sinks; each is opt-in by config, and both skip content already uploaded.

mod parquet_sink;

use std::path::PathBuf;
use std::time::Duration;

use anyhow::Context as _;
use source_claude::ClaudeHistoryExport;
use clap::Parser;
use search_core::{MixedbreadStore, sync_documents};

/// How long to wait for Mixedbread to finish embedding new documents.
const INDEX_TIMEOUT: Duration = Duration::from_mins(2);

/// Sync Claude Code agent transcripts to an S3/R2 parquet archive and/or a
/// Mixedbread store.
#[derive(Debug, Parser)]
#[command(name = "claude-history-sync", about, version)]
struct Cli {
    /// Transcript directory (default: `~/.claude/projects`).
    #[arg(long)]
    dir: Option<PathBuf>,

    /// Bucket for the parquet archive; enables the S3/R2 sink. Credentials come
    /// from `AWS_ACCESS_KEY_ID` / `AWS_SECRET_ACCESS_KEY`.
    #[arg(long, env = "CLAUDE_HISTORY_R2_BUCKET")]
    r2_bucket: Option<String>,

    /// S3 endpoint URL; empty means AWS S3, for R2 pass the account endpoint.
    #[arg(long, env = "CLAUDE_HISTORY_R2_ENDPOINT")]
    r2_endpoint: Option<String>,

    /// S3 region (`auto` for R2).
    #[arg(long, env = "CLAUDE_HISTORY_R2_REGION", default_value = "auto")]
    r2_region: String,

    /// Key prefix under the bucket.
    #[arg(long, env = "CLAUDE_HISTORY_PREFIX", default_value = "claude-history")]
    prefix: String,

    /// Mixedbread store name; enables the Mixedbread sink.
    #[arg(long, env = "MXBAI_STORE")]
    mixedbread_store: Option<String>,

    /// Mixedbread API base URL.
    #[arg(long = "base-url", env = "MXBAI_BASE_URL")]
    base_url: Option<String>,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();

    if cli.r2_bucket.is_none() && cli.mixedbread_store.is_none() {
        anyhow::bail!("nothing to do: pass --r2-bucket and/or --mixedbread-store");
    }

    let dir = match cli.dir {
        Some(dir) => dir,
        None => default_dir()?,
    };
    let export = ClaudeHistoryExport::open(&dir).context("parsing Claude transcripts")?;
    eprintln!("parsed {} messages from {}", export.len(), dir.display());

    if let Some(store_name) = &cli.mixedbread_store {
        let base_url = cli
            .base_url
            .clone()
            .unwrap_or_else(|| mixedbread::DEFAULT_BASE_URL.to_owned());
        let store = MixedbreadStore::from_login(base_url)
            .await
            .context("connecting to Mixedbread")?;
        let report = sync_documents(&export, &store, store_name, INDEX_TIMEOUT, |_, _| {})
            .await
            .context("Mixedbread sync")?;
        eprintln!(
            "mixedbread: uploaded {}, skipped {} of {}",
            report.uploaded, report.skipped, report.total
        );
    }

    if let Some(bucket) = &cli.r2_bucket {
        let config = parquet_sink::Config {
            bucket: bucket.clone(),
            endpoint: cli.r2_endpoint.clone(),
            region: cli.r2_region.clone(),
            prefix: cli.prefix.clone(),
            host: hostname(),
        };
        let report = parquet_sink::sync(export.messages(), &config)
            .await
            .context("R2 parquet sync")?;
        eprintln!("r2: uploaded {} sessions, skipped {}", report.uploaded, report.skipped);
    }

    Ok(())
}

/// Default transcript directory: `~/.claude/projects`.
fn default_dir() -> anyhow::Result<PathBuf> {
    let home = dirs::home_dir().context("no home directory")?;
    Ok(home.join(".claude").join("projects"))
}

/// Short hostname for the manifest key and the `host=` parquet partition.
fn hostname() -> String {
    nix::unistd::gethostname()
        .ok()
        .map_or_else(|| "unknown".to_owned(), |name| name.to_string_lossy().into_owned())
}
