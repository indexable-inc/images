use std::sync::Arc;

use clap::Parser;
use deadpool_postgres::{Manager, Pool};
use snafu::{OptionExt, ResultExt};
use tokio_postgres::NoTls;

use subagent_cache::config::Config;
use subagent_cache::error::{
    BindSnafu, DbConfigSnafu, MissingApiKeySnafu, PoolBuildSnafu, Result, ServeSnafu,
};
use subagent_cache::http::{AppState, router};
use subagent_cache::store;

/// Connection pool ceiling. The daemon is low-traffic (one call per subagent
/// launch), so a small pool is ample.
const MAX_DB_CONNECTIONS: usize = 8;

#[snafu::report]
#[tokio::main]
async fn main() -> Result<()> {
    // JSON logs to stderr: structured `target` + `fields` per event. A log
    // shipper that ingests the systemd journal (e.g. the ix fleet's Vector ->
    // ClickHouse pipeline, which parses a JSON MESSAGE into typed columns) gets
    // the `target`, `agent_type`, and `outcome` fields the cache-observability
    // dashboards key on, instead of an opaque text line.
    tracing_subscriber::fmt()
        .json()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .init();

    let config = Config::parse();

    // Read once at startup so a misconfigured deploy fails fast rather than on
    // the first lookup. Never placed on argv.
    let api_key: Arc<str> = std::env::var("ANTHROPIC_API_KEY")
        .ok()
        .filter(|k| !k.is_empty())
        .context(MissingApiKeySnafu)?
        .into();

    // NoTls: the daemon co-locates with its dedicated Postgres and reaches it
    // over the trusted host/vrack address, never the public internet.
    let pg_config: tokio_postgres::Config =
        config.database_url.parse().context(DbConfigSnafu)?;
    let manager = Manager::new(pg_config, NoTls);
    let pool: Pool = Pool::builder(manager)
        .max_size(MAX_DB_CONNECTIONS)
        .build()
        .context(PoolBuildSnafu)?;
    store::bootstrap(&pool).await?;

    let bind = config.bind;
    let state = AppState {
        pool,
        http: reqwest::Client::new(),
        api_key,
        config: Arc::new(config),
    };

    let listener = tokio::net::TcpListener::bind(bind)
        .await
        .context(BindSnafu { bind })?;
    tracing::info!(%bind, "subagent-cache listening");

    // Drain in-flight requests on SIGINT (ctrl_c). systemd's SIGTERM stop is an
    // abrupt kill here, which is fine: the cache is fail-open, so a hard stop
    // only costs cold runs, never correctness.
    axum::serve(listener, router(state))
        .with_graceful_shutdown(async {
            let _ = tokio::signal::ctrl_c().await;
            tracing::info!("shutting down");
        })
        .await
        .context(ServeSnafu)?;
    Ok(())
}
