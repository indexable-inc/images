//! Typed errors for the daemon. snafu only; source errors are preserved and
//! never interpolated into the display string.

use snafu::Snafu;

#[derive(Debug, Snafu)]
#[snafu(visibility(pub))]
pub enum Error {
    #[snafu(display("the cache database URL is not a valid Postgres connection string"))]
    DbConfig { source: tokio_postgres::Error },

    #[snafu(display("failed to build the cache database connection pool"))]
    PoolBuild { source: deadpool_postgres::BuildError },

    #[snafu(display("failed to acquire a cache database connection from the pool"))]
    Pool { source: deadpool_postgres::PoolError },

    #[snafu(display("failed to apply the cache schema"))]
    Schema { source: tokio_postgres::Error },

    #[snafu(display("recall query failed for agent_type {agent_type}"))]
    Recall {
        agent_type: String,
        source: tokio_postgres::Error,
    },

    #[snafu(display("populate upsert failed for agent_type {agent_type}"))]
    Populate {
        agent_type: String,
        source: tokio_postgres::Error,
    },

    #[snafu(display("failed to encode file_deps as JSON"))]
    JsonEncode { source: serde_json::Error },

    #[snafu(display("a stored file_deps value could not be decoded"))]
    JsonDecode { source: serde_json::Error },

    #[snafu(display("the judge request to {model} failed to send"))]
    JudgeSend {
        model: String,
        source: reqwest::Error,
    },

    #[snafu(display("the judge returned HTTP {status}: {body}"))]
    JudgeStatus { status: u16, body: String },

    #[snafu(display("the judge response could not be decoded"))]
    JudgeDecode { source: reqwest::Error },

    #[snafu(display("the ANTHROPIC_API_KEY environment variable is not set"))]
    MissingApiKey,

    #[snafu(display("failed to bind the HTTP listener on {bind}"))]
    Bind {
        bind: std::net::SocketAddr,
        source: std::io::Error,
    },

    #[snafu(display("the HTTP server exited with an error"))]
    Serve { source: std::io::Error },
}

pub type Result<T, E = Error> = std::result::Result<T, E>;
