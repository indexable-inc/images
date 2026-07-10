//! axum HTTP surface: `/lookup`, `/populate`, `/healthz`, plus the lookup
//! instrumentation that lets v1 measure its own hit rate.

use std::sync::Arc;

use axum::Json;
use axum::Router;
use axum::extract::State;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use deadpool_postgres::Pool;

use crate::config::Config;
use crate::error::Error;
use crate::types::{Candidate, LookupRequest, LookupResponse, OutcomeRequest, PopulateRequest};
use crate::{judge, store};

#[derive(Clone)]
pub struct AppState {
    pub pool: Pool,
    pub http: reqwest::Client,
    pub api_key: Arc<str>,
    pub config: Arc<Config>,
}

pub fn router(state: AppState) -> Router {
    Router::new()
        .route("/healthz", get(|| async { "ok" }))
        .route("/lookup", post(lookup))
        .route("/outcome", post(outcome))
        .route("/populate", post(populate))
        .with_state(state)
}

async fn lookup(
    State(state): State<AppState>,
    Json(req): Json<LookupRequest>,
) -> Result<Json<LookupResponse>, Error> {
    let cfg = &state.config;
    let recalled = store::recall(
        &state.pool,
        store::RecallParams {
            prompt: &req.prompt,
            agent_type: &req.agent_type,
            agent_def_hash: &req.agent_def_hash,
            floor: cfg.recall_floor,
            top_k: cfg.recall_top_k,
        },
    )
    .await?;

    if recalled.is_empty() {
        tracing::info!(
            target: "subagent_cache.lookup",
            agent_type = %req.agent_type,
            recalled = 0,
            judged = 0,
            result = "miss",
            "lookup miss: no candidate cleared the recall floor",
        );
        return Ok(Json(LookupResponse {
            candidates: Vec::new(),
        }));
    }

    let accepted = judge::judge(
        &judge::JudgeApi {
            http: &state.http,
            api_base: &cfg.judge_api_base,
            api_key: &state.api_key,
            model: &cfg.judge_model,
        },
        &req.prompt,
        &recalled,
    )
    .await?;

    let candidates: Vec<Candidate> = accepted
        .iter()
        .filter_map(|&i| recalled.get(i))
        .map(|row| Candidate {
            id: row.id,
            findings: row.findings.clone(),
            file_deps: row.file_deps.clone(),
        })
        .collect();

    tracing::info!(
        target: "subagent_cache.lookup",
        agent_type = %req.agent_type,
        recalled = recalled.len(),
        judged = candidates.len(),
        result = if candidates.is_empty() { "miss" } else { "hit" },
        "lookup complete (client validates freshness)",
    );

    Ok(Json(LookupResponse { candidates }))
}

async fn outcome(Json(req): Json<OutcomeRequest>) -> StatusCode {
    let candidate_id = req
        .candidate_id
        .map(|id| id.to_string())
        .unwrap_or_default();
    tracing::info!(
        target: "subagent_cache.outcome",
        agent_type = %req.agent_type,
        outcome = req.outcome.as_str(),
        candidate_id = %candidate_id,
        "lookup outcome resolved by client",
    );
    StatusCode::NO_CONTENT
}

async fn populate(
    State(state): State<AppState>,
    Json(req): Json<PopulateRequest>,
) -> Result<StatusCode, Error> {
    store::populate(&state.pool, &req, state.config.ttl_days).await?;
    tracing::info!(
        target: "subagent_cache.populate",
        agent_type = %req.agent_type,
        deps = req.file_deps.len(),
        "populated cache entry",
    );
    Ok(StatusCode::NO_CONTENT)
}

impl IntoResponse for Error {
    fn into_response(self) -> Response {
        // Every variant is an internal failure from the client's view; the
        // hook fails open to a cold run, so the body is for operators only.
        tracing::error!(error = %snafu::Report::from_error(self), "request failed");
        (StatusCode::INTERNAL_SERVER_ERROR, "internal error").into_response()
    }
}
