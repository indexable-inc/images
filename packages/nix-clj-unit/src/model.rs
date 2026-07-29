//! The `render` output schema, as consumed by the Nix side.

use std::collections::BTreeMap;

/// Bumped whenever the shape below changes incompatibly, so a stale generated
/// graph is rejected rather than silently misread.
pub const SCHEMA_VERSION: u32 = 1;

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct Graph {
    pub version: u32,
    /// Keyed by namespace. A `BTreeMap` because the JSON must be
    /// byte-identical across runs, and `serde_json` writes map keys in iteration
    /// order.
    pub namespaces: BTreeMap<String, Namespace>,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct Namespace {
    /// Source path, relative to the source root it was found under and
    /// prefixed with that root exactly as it was written on the command line.
    pub file: String,
    /// Namespaces this one requires that are themselves units in this graph,
    /// sorted and deduplicated.
    pub requires: Vec<String>,
}
