//! `otlp-mixedbread`: receive OTLP logs over HTTP and index them into Mixedbread.
//!
//! Point the OpenTelemetry collector's `otlphttp` exporter (JSON encoding) at
//! this service's `/v1/logs`; every log record becomes a semantically searchable
//! document. Mixedbread auth comes from `MXBAI_API_KEY` (else the `mgrep login`
//! token), the same as the `indexer`.

use std::net::SocketAddr;
use std::sync::Arc;

use anyhow::Context as _;
use clap::Parser;
use search_core::{MixedbreadStore, Store as _};
use tracing_subscriber::EnvFilter;

use otlp_mixedbread::{AppState, Config, router, spawn};

/// Receive OTLP logs over HTTP and index them into a Mixedbread store.
#[derive(Debug, Parser)]
#[command(name = "otlp-mixedbread", about, version)]
struct Cli {
    /// Mixedbread store to index log records into.
    #[arg(long, env = "MXBAI_STORE")]
    store: String,

    /// Mixedbread API base URL override. Absent uses the SDK default.
    #[arg(long = "base-url", env = "MXBAI_BASE_URL")]
    base_url: Option<String>,

    /// Address to serve the OTLP/HTTP receiver on. Defaults to loopback so the
    /// co-located collector reaches it without exposing it on the network.
    #[arg(long, env = "OTLP_MIXEDBREAD_LISTEN", default_value = "127.0.0.1:4319")]
    listen: SocketAddr,

    /// The `source` tag stamped on every document (the corpus name).
    #[arg(long = "source-tag", default_value = "log")]
    source_tag: String,

    /// Drop records whose OTLP severity number is below this (1..=24, higher is
    /// more severe; 13 = WARN). 0 keeps everything. A floor in addition to any
    /// filtering the collector pipeline already applies.
    #[arg(long = "min-severity-number", default_value_t = 0)]
    min_severity: i32,

    /// Maximum concurrent uploads to Mixedbread.
    #[arg(long, default_value_t = 8)]
    concurrency: usize,

    /// Bounded queue depth between the HTTP handler and the upload workers. When
    /// full, the handler returns 503 so the collector retries.
    #[arg(long = "queue-capacity", default_value_t = 10_000)]
    queue_capacity: usize,

    /// How many recent record ids to remember to skip re-embedding on retries.
    #[arg(long = "dedup-capacity", default_value_t = 100_000)]
    dedup_capacity: usize,

    /// Upload attempts per record before giving up (and logging the failure).
    #[arg(long = "max-upload-attempts", default_value_t = 5)]
    max_attempts: u32,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();

    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")))
        .init();

    let base_url = cli.base_url.unwrap_or_else(|| mixedbread::DEFAULT_BASE_URL.to_owned());
    let store = MixedbreadStore::from_login(base_url).await.context("connecting to Mixedbread")?;
    store.ensure_store(&cli.store).await.context("ensuring the Mixedbread store exists")?;

    let ingest = spawn(
        Arc::new(store),
        Arc::from(cli.store.as_str()),
        Config {
            queue_capacity: cli.queue_capacity,
            concurrency: cli.concurrency,
            dedup_capacity: cli.dedup_capacity,
            max_attempts: cli.max_attempts,
        },
    );

    let app = router(AppState {
        ingest,
        source: Arc::from(cli.source_tag.as_str()),
        min_severity: cli.min_severity,
    });

    let listener = tokio::net::TcpListener::bind(cli.listen)
        .await
        .with_context(|| format!("binding {}", cli.listen))?;
    tracing::info!(listen = %cli.listen, store = %cli.store, "otlp-mixedbread ready");
    axum::serve(listener, app)
        .with_graceful_shutdown(shutdown_signal())
        .await
        .context("serving OTLP/HTTP")?;
    Ok(())
}

/// Resolve when the process receives Ctrl-C or `SIGTERM` (the systemd stop
/// signal), so in-flight uploads can drain on shutdown.
async fn shutdown_signal() {
    let ctrl_c = async {
        let _ = tokio::signal::ctrl_c().await;
    };
    #[cfg(unix)]
    let terminate = async {
        match tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate()) {
            Ok(mut term) => {
                term.recv().await;
            }
            Err(error) => tracing::warn!(%error, "failed to install SIGTERM handler"),
        }
    };
    #[cfg(not(unix))]
    let terminate = std::future::pending::<()>();

    tokio::select! {
        () = ctrl_c => {}
        () = terminate => {}
    }
    tracing::info!("shutdown signal received");
}
