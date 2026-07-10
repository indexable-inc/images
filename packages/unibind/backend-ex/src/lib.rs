//! Render a lowered [`unibind_core::ir::Interface`] into `rustler` binding
//! code and the Elixir modules that call it.
//!
//! The backend targets the incumbent binding library rather than raw FFI:
//! it emits `#[rustler::nif]` wrappers around the user's functions, derives
//! `NifStruct` onto record structs, registers objects as BEAM resources,
//! and hands async functions and streams to `unibind-ex-runtime`. The
//! consuming crate therefore depends on `rustler` and `unibind-ex-runtime`
//! directly and builds a `cdylib` that `:erlang.load_nif/2` loads. The
//! matching Elixir side (`<Ns>.Native` stubs and the typespec'd `<Ns>`
//! wrapper) comes from [`host_modules`], which `unibind-gen`'s `ExEmitter`
//! writes to disk.

mod error;
mod function;
mod host;
mod module;
mod names;
mod object;
mod record;
mod ty;

pub use host::{host_modules, HostModules};
pub use module::render;

/// The rendered output for one interface.
pub struct RenderedInterface {
    /// Sibling items for the exported module: the hidden glue module with
    /// the error terms, resource registrations, NIF wrappers, and the
    /// `rustler::init!` invocation.
    pub glue: proc_macro2::TokenStream,
    /// Attributes to attach to each record struct, index-aligned with the
    /// interface's records.
    pub records: Vec<RenderedRecord>,
}

/// `NifStruct`-shaped attributes for one record struct.
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
