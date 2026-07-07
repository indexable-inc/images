//! Render a lowered [`unibind_core::ir::Interface`] into `swift-bridge`
//! binding code.
//!
//! The backend targets the incumbent Rust-Swift bridge rather than raw FFI:
//! it emits a genuine `#[swift_bridge::bridge]` module, so the FFI symbols
//! match swift-bridge's Swift output by construction. Records cross as
//! opaque handles with per-field getters and a field-by-field constructor
//! (swift-bridge shared structs cannot hold maps or vectors of
//! non-primitives), containers the bridge cannot express directly (maps,
//! vectors of strings or records, options of composites) cross as opaque
//! box handles with index accessors, and error enums cross as transparent
//! enums whose variants carry the Rust `Display` text. The ergonomic Swift
//! surface -- records as Swift structs, errors as thrown Swift enums,
//! lowerCamelCase free functions with default arguments and documentation
//! comments -- is a rendered overlay ([`RenderedInterface::overlay`]) that
//! converts between native Swift types and the bridge handles.

mod boxes;
mod error;
mod function;
mod module;
mod names;
mod record;
mod repr;
mod swift;
mod swift_convert;

pub use module::render;

/// The rendered output for one interface.
#[derive(Debug)]
pub struct RenderedInterface {
    /// Sibling item for the exported module: the hidden glue module holding
    /// the bridge module, the record and box handle types, and the wrapper
    /// functions the bridge dispatches to.
    pub glue: proc_macro2::TokenStream,
    /// The bare `#[swift_bridge::bridge]` module (also embedded in `glue`).
    /// `unibind-gen` writes it to a standalone file for `swift-bridge-build`,
    /// which only scans top-level items, to derive the low-level Swift and C
    /// header from exactly the tokens the macro expanded.
    pub bridge: proc_macro2::TokenStream,
    /// The ergonomic Swift overlay source, compiled into the same Swift
    /// module as swift-bridge's generated output.
    pub overlay: String,
}

/// A rendering failure; the macro positions it at the exported module.
#[derive(Debug)]
pub struct RenderError {
    /// What went wrong and what to do instead.
    pub message: String,
}

impl RenderError {
    pub(crate) fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
        }
    }
}
