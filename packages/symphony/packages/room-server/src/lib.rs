// Library surface for the room server. Exposed as a crate so the
// integration tests under `tests/` can drive the same modules the
// binary uses without duplicating the pipeline.

pub mod agent;
pub mod annotations;
pub mod codex_bridge;
pub mod codex_rpc;
pub mod db;
pub mod engine;
pub mod engine_claude;
pub mod engine_codex;
pub mod engine_handle;
pub mod http;
pub mod state;
pub mod tool_result;
pub mod workspace;
pub mod wt;
