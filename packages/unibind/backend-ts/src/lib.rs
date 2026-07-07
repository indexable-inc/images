//! Render a lowered [`unibind_core::ir::Interface`] into `napi-rs` binding
//! code.
//!
//! The backend targets the incumbent binding library rather than raw FFI:
//! it emits `#[napi]` wrappers around the user's functions, attaches
//! `#[napi(object)]` to record structs, converts error enums into
//! machine-decodable `napi::Error` reasons, and wraps streams and objects
//! in generated handle classes. The consuming crate therefore depends on
//! `napi` (features `napi6` + `tokio_rt`), `napi-derive`, `tokio` (features
//! `sync` + `macros`), and `unibind-runtime` directly, and builds a cdylib
//! with a `napi_build::setup()` build script.
//!
//! Everything dynamic crosses to JavaScript through the `napi::Error`
//! reason string, prefixed with `__unibind__:` (see [`error`]); the
//! generated `index.js` (`unibind-gen ts`) decodes it into real `Error`
//! subclasses, wraps stream handles into `AsyncIterable`s, and
//! materializes the enriched `.d.ts` from the embedded IR.

mod defaults;
mod error;
mod function;
mod module;
mod object;
mod record;
mod stream;
mod ty;

pub use module::render;

/// The rendered output for one interface.
pub struct RenderedInterface {
    /// Sibling items for the exported module: the hidden glue module with
    /// the `From` error impls, `#[napi]` wrappers, and the stream and
    /// object handle classes. napi registers exports through link-time
    /// constructors, so there is no explicit module registration item.
    pub glue: proc_macro2::TokenStream,
    /// Attributes to attach to each record struct, index-aligned with the
    /// interface's records.
    pub records: Vec<RenderedRecord>,
}

/// `#[napi(object)]`-shaped attributes for one record struct.
pub struct RenderedRecord {
    /// Outer attributes for the struct itself.
    pub outer: Vec<syn::Attribute>,
    /// Attributes for each field, index-aligned with the record's fields.
    pub fields: Vec<Vec<syn::Attribute>>,
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
