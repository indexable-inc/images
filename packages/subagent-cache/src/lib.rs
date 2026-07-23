//! Content-validated cache for subagent investigations (ENG-4665).
//!
//! The daemon owns Stage-1 FTS recall and the Stage-2 Haiku judge over a
//! dedicated Postgres. Stage-3 freshness (re-hashing dependency files) and the
//! persona check live in the client hook, because only the client can see its
//! working tree. See `04-plan-subagent-cache.md` for the protocol contract.

pub mod config;
pub mod error;
pub mod http;
pub mod judge;
pub mod store;
pub mod types;
