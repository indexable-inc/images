//! The OTLP/HTTP front door: a `POST /v1/logs` receiver and a `/health` probe.
//!
//! We accept the OTLP logs export in JSON encoding (the collector's `otlphttp`
//! exporter with `encoding: json`). Compressed bodies are rejected with `415`;
//! configure the exporter with `compression: none` (the collector and this
//! service share a host or a local network, so compression buys little). On a
//! full upload queue the handler returns `503` so the collector retries with
//! backoff rather than this service buffering without bound.

use std::sync::Arc;

use axum::Router;
use axum::body::Bytes;
use axum::extract::State;
use axum::http::{HeaderMap, StatusCode, header};
use axum::response::IntoResponse;
use axum::routing::{get, post};

use crate::ingest::{Ingest, Sent};
use crate::otlp::{ExportLogsServiceRequest, severity_number};
use crate::project::project;

/// Shared handler state.
#[derive(Clone)]
pub struct AppState {
    /// The upload pipeline handle.
    pub ingest: Arc<Ingest>,
    /// The `source` tag stamped on every document (the corpus name).
    pub source: Arc<str>,
    /// Drop records whose OTLP severity number is below this (0 keeps all). A
    /// defense-in-depth floor; the collector pipeline is the primary filter.
    pub min_severity: i32,
}

/// Build the router for the OTLP logs receiver.
#[must_use]
pub fn router(state: AppState) -> Router {
    Router::new()
        .route("/v1/logs", post(ingest_logs))
        .route("/health", get(health))
        .with_state(state)
}

/// Liveness probe.
async fn health() -> StatusCode {
    StatusCode::OK
}

/// Receive one OTLP logs export, project each record, and enqueue it.
async fn ingest_logs(
    State(state): State<AppState>,
    headers: HeaderMap,
    body: Bytes,
) -> impl IntoResponse {
    if is_compressed(&headers) {
        return (
            StatusCode::UNSUPPORTED_MEDIA_TYPE,
            "compressed bodies are not supported; set the exporter `compression: none`",
        )
            .into_response();
    }

    let request: ExportLogsServiceRequest = match serde_json::from_slice(&body) {
        Ok(request) => request,
        Err(error) => {
            return (StatusCode::BAD_REQUEST, format!("invalid OTLP/JSON logs body: {error}"))
                .into_response();
        }
    };

    let mut accepted = 0_u64;
    let mut dropped = 0_u64;
    // A full queue aborts the batch (we have not consumed it), so the collector
    // retries the whole export; dedup keeps the retry from re-embedding.
    for resource_logs in &request.resource_logs {
        for scope_logs in &resource_logs.scope_logs {
            for record in &scope_logs.log_records {
                if below_floor(record.severity_number.as_ref(), state.min_severity) {
                    dropped += 1;
                    continue;
                }
                let Some(document) = project(resource_logs.resource.as_ref(), record, &state.source)
                else {
                    dropped += 1;
                    continue;
                };
                match state.ingest.offer(document) {
                    Sent::Accepted => accepted += 1,
                    Sent::Full => return backpressure(accepted),
                    Sent::Closed => {
                        return (StatusCode::SERVICE_UNAVAILABLE, "shutting down").into_response();
                    }
                }
            }
        }
    }

    tracing::debug!(accepted, dropped, "accepted OTLP logs batch");
    // An empty `ExportLogsServiceResponse` signals full success to the collector.
    (StatusCode::OK, [(header::CONTENT_TYPE, "application/json")], "{}").into_response()
}

/// Tell the collector to retry: queue is full. `Retry-After` is advisory.
fn backpressure(accepted_before_full: u64) -> axum::response::Response {
    tracing::warn!(accepted_before_full, "upload queue full, asking collector to retry");
    (StatusCode::SERVICE_UNAVAILABLE, [(header::RETRY_AFTER, "1")], "upload queue full")
        .into_response()
}

/// Whether the request carries a non-identity `Content-Encoding`.
fn is_compressed(headers: &HeaderMap) -> bool {
    headers.get(header::CONTENT_ENCODING).is_some_and(|value| {
        !value.as_bytes().eq_ignore_ascii_case(b"identity") && !value.is_empty()
    })
}

/// Whether a record falls below the severity floor. A record with no severity is
/// kept (we do not drop unlabeled logs); a floor of 0 keeps everything.
fn below_floor(severity: Option<&serde_json::Value>, floor: i32) -> bool {
    floor > 0 && severity_number(severity).is_some_and(|number| number < floor)
}
