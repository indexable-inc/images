//! Daemon configuration, all overridable via CLI flags or environment.

use std::net::SocketAddr;

/// Runtime configuration for the subagent-cache daemon.
///
/// The recall floor and top-K are deliberately config: the design fixes their
/// shape but not their values, which must be tuned against measured hit and
/// false-hit rates rather than asserted.
#[derive(Debug, Clone, clap::Parser)]
#[command(name = "subagent-cache", about = "Content-validated subagent investigation cache")]
pub struct Config {
    /// Postgres connection URL for the dedicated cache database.
    #[arg(long, env = "DATABASE_URL")]
    pub database_url: String,

    /// Address to bind the HTTP server on.
    #[arg(long, env = "SUBAGENT_CACHE_BIND", default_value = "127.0.0.1:8787")]
    pub bind: SocketAddr,

    /// Number of FTS candidates the judge may inspect per lookup.
    #[arg(long, env = "SUBAGENT_CACHE_TOP_K", default_value_t = 3)]
    pub recall_top_k: i64,

    /// Minimum `ts_rank` a candidate must score to reach the judge. Below it,
    /// the lookup is a cheap miss (the judge never fires).
    #[arg(long, env = "SUBAGENT_CACHE_RECALL_FLOOR", default_value_t = 0.01)]
    pub recall_floor: f32,

    /// TTL backstop in days. An entry expires this long after its last
    /// populate even if none of its files ever change.
    #[arg(long, env = "SUBAGENT_CACHE_TTL_DAYS", default_value_t = 7)]
    pub ttl_days: i64,

    /// Anthropic Messages API base URL (overridable for tests/proxies).
    #[arg(long, env = "SUBAGENT_CACHE_JUDGE_API_BASE", default_value = "https://api.anthropic.com")]
    pub judge_api_base: String,

    /// Judge model id. A Haiku-class model; overridable as model ids roll.
    #[arg(long, env = "SUBAGENT_CACHE_JUDGE_MODEL", default_value = "claude-haiku-4-5")]
    pub judge_model: String,
}
