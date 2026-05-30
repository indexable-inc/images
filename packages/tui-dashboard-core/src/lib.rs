//! Typed resource model and Loro-backed hub for the MCP dashboard.
//!
//! A [`Hub`] holds a Loro CRDT document and stores each producer's resources
//! under its own scope, retaining history for time-travel. Producers publish a
//! [`ProducerSnapshot`] of typed [`Resource`] values; [`render`] turns each
//! resource into an HTML fragment the dashboard shell can slot in and update in
//! place, so a new browser page auto-appears the way a terminal does today.
//!
//! This is milestone 1 of ENG-1865: the typed [`Resource`] sum behind the old
//! single terminal kind, plus the [`Resource::BrowserPage`] variant and a
//! per-kind renderer. Future `Image`, `Table`, `LogStream`, and `Text` kinds
//! attach as new variants.

#![allow(
    clippy::missing_errors_doc,
    reason = "fallible hub methods document their failures through the typed `SnapshotError` enum"
)]

mod hub;
mod render;
mod resource;

pub use hub::{Hub, ProducerId, ProducerSnapshot, SnapshotError};
pub use render::{RenderedResource, render};
pub use resource::{
    BrowserPage, ImageMediaType, Resource, ResourceId, ResourceKind, Screenshot, TerminalFrame,
    TerminalSize,
};
