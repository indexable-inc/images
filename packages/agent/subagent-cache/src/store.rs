//! Postgres-backed storage: schema bootstrap, Stage-1 FTS recall, and populate.
//!
//! `tokio-postgres` over a `deadpool` pool (no sqlx: its sqlite driver links the
//! `sqlite3` native lib and collides with `rusqlite` elsewhere in this
//! workspace). Plain runtime queries, so the crate builds without a live
//! database or an offline query cache.

use chrono::{DateTime, Utc};
use deadpool_postgres::Pool;
use snafu::ResultExt;

use crate::error::{JsonDecodeSnafu, JsonEncodeSnafu, PoolSnafu, PopulateSnafu, RecallSnafu, Result, SchemaSnafu};
use crate::types::{FileDep, PopulateRequest};

const SCHEMA: &str = include_str!("../schema.sql");

/// One FTS candidate before judging and freshness validation.
#[derive(Debug, Clone)]
pub struct RecallRow {
    pub id: uuid::Uuid,
    pub question: String,
    pub findings: String,
    pub file_deps: Vec<FileDep>,
    pub score: f32,
}

/// Apply the schema. Idempotent: safe to run on every startup.
///
/// # Errors
/// Errors if a pooled connection cannot be acquired or the schema statements
/// fail to execute.
pub async fn bootstrap(pool: &Pool) -> Result<()> {
    let client = pool.get().await.context(PoolSnafu)?;
    client.batch_execute(SCHEMA).await.context(SchemaSnafu)?;
    Ok(())
}

/// Inputs to a Stage-1 recall query.
#[derive(Debug, Clone, Copy)]
pub struct RecallParams<'a> {
    pub prompt: &'a str,
    pub agent_type: &'a str,
    /// Persona hash; folds the persona check into the recall filter.
    pub agent_def_hash: &'a str,
    /// Minimum `ts_rank` for a candidate to reach the judge.
    pub floor: f32,
    /// Max candidates the judge may inspect.
    pub top_k: i64,
}

/// Stage 1: rank non-expired rows of the same `agent_type` and persona by
/// full-text relevance, returning the top-K above the recall floor, best-first.
///
/// # Errors
/// Errors if a pooled connection cannot be acquired, the recall query fails, or
/// a stored `file_deps` value cannot be decoded.
pub async fn recall(pool: &Pool, params: RecallParams<'_>) -> Result<Vec<RecallRow>> {
    let client = pool.get().await.context(PoolSnafu)?;
    let rows = client
        .query(
            "SELECT id, question, findings, file_deps, \
                    ts_rank(question_tsv, plainto_tsquery('english', $1)) AS score \
             FROM subagent_cache \
             WHERE agent_type = $2 AND agent_def_hash = $3 AND expires_at > now() \
               AND question_tsv @@ plainto_tsquery('english', $1) \
             ORDER BY score DESC \
             LIMIT $4",
            &[
                &params.prompt,
                &params.agent_type,
                &params.agent_def_hash,
                &params.top_k,
            ],
        )
        .await
        .context(RecallSnafu { agent_type: params.agent_type.to_owned() })?;

    let mut out = Vec::with_capacity(rows.len());
    for row in rows {
        let score: f32 = row.get("score");
        // The floor gates whether the judge fires at all; apply it after the
        // ranked fetch so the SQL evaluates `ts_rank` only once.
        if score < params.floor {
            continue;
        }
        let file_deps_json: serde_json::Value = row.get("file_deps");
        let file_deps: Vec<FileDep> = serde_json::from_value(file_deps_json).context(JsonDecodeSnafu)?;
        out.push(RecallRow {
            id: row.get("id"),
            question: row.get("question"),
            findings: row.get("findings"),
            file_deps,
            score,
        });
    }
    Ok(out)
}

/// Upsert one finished investigation onto the (`agent_type`, question, persona)
/// key. The TTL backstop is computed here and stored as an absolute instant.
///
/// # Errors
/// Errors if `file_deps` cannot be encoded, a pooled connection cannot be
/// acquired, or the upsert fails.
pub async fn populate(pool: &Pool, req: &PopulateRequest, ttl_days: i64) -> Result<()> {
    let expires_at: DateTime<Utc> = Utc::now() + chrono::Duration::days(ttl_days);
    let file_deps = serde_json::to_value(&req.file_deps).context(JsonEncodeSnafu)?;
    let client = pool.get().await.context(PoolSnafu)?;
    client
        .execute(
            "INSERT INTO subagent_cache \
                (agent_type, question, findings, agent_def_hash, model, file_deps, expires_at) \
             VALUES ($1, $2, $3, $4, $5, $6, $7) \
             ON CONFLICT (agent_type, question, agent_def_hash) DO UPDATE SET \
                findings = EXCLUDED.findings, \
                model = EXCLUDED.model, \
                file_deps = EXCLUDED.file_deps, \
                created_at = now(), \
                expires_at = EXCLUDED.expires_at",
            &[
                &req.agent_type,
                &req.prompt,
                &req.findings,
                &req.agent_def_hash,
                &req.model,
                &file_deps,
                &expires_at,
            ],
        )
        .await
        .context(PopulateSnafu { agent_type: req.agent_type.clone() })?;
    Ok(())
}
