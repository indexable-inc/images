//! Golden-fixture tests for the cross-language engine contract.
//!
//! These parse the shared fixtures under `contracts/fixtures/` (the same
//! files the Elixir side parses) and re-serialize them, so a field rename
//! that breaks the wire shape fails here instead of at runtime. See
//! `docs/engine-contract.md`.

use room_server::agent::AgentTurnResponse;
use room_server::engine::{
    EngineEvent, EngineEventBody, EngineKind, Permissions, TurnOutcome, TurnRequest,
};

fn fixture(name: &str) -> String {
    // CARGO_MANIFEST_DIR is packages/room-server; fixtures live at the repo root.
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../contracts/fixtures")
        .join(name);
    std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("read {}: {e}", path.display()))
}

#[test]
fn turn_request_fixture_parses() {
    let req: TurnRequest = serde_json::from_str(&fixture("turn_request.json")).unwrap();
    assert_eq!(req.engine, EngineKind::Codex);
    assert_eq!(req.model, "gpt-5.3-codex");
    assert_eq!(req.permissions, Permissions::WorkspaceWrite);
    assert_eq!(req.run_id.as_deref(), Some("run_x"));

    // Re-serializing keeps the wire shape (camelCase, tagged).
    let json = serde_json::to_string(&req).unwrap();
    assert!(json.contains("\"runId\":\"run_x\""));
}

#[test]
fn agent_turn_response_fixture_parses() {
    let resp: AgentTurnResponse =
        serde_json::from_str(&fixture("agent_turn_response.json")).unwrap();
    assert_eq!(resp.thread_id, "thread_abc");
    assert_eq!(resp.outcome, TurnOutcome::Ok);
    assert_eq!(resp.event_count, 4);
    assert_eq!(resp.usage.tokens_in, 1200);
    assert_eq!(resp.usage.tokens_out, 340);
    assert_eq!(resp.usage.cache_read, 800);
    assert_eq!(resp.usage.cache_creation, 64);
    assert_eq!(resp.usage.cost_usd, Some(0.0123));

    // Re-serializing keeps the camelCase wire shape Elixir parses.
    let json = serde_json::to_string(&resp).unwrap();
    assert!(json.contains("\"costUsd\":0.0123"));
    assert!(json.contains("\"tokensIn\":1200"));
}

#[test]
fn engine_event_fixture_parses() {
    let event: EngineEvent = serde_json::from_str(&fixture("engine_event.json")).unwrap();
    assert_eq!(event.turn_id, "thread_abc");
    assert_eq!(event.seq, 7);
    assert!(matches!(event.body, EngineEventBody::TextDelta { .. }));
}
