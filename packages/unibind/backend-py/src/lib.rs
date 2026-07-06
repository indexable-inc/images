//! Render a lowered [`unibind_core::ir::Interface`] into `pyo3` binding code.
//!
//! The backend targets the incumbent binding library rather than raw FFI: it
//! emits `#[pyo3::pyfunction]` wrappers around the user's functions, attaches
//! `#[pyo3::pyclass]` to record structs, builds the exception hierarchy with
//! `pyo3::create_exception!`, and registers everything in one imperative
//! `#[pyo3::pymodule]`. The consuming crate therefore depends on `pyo3`
//! directly (with `extension-module` for a wheel-shaped cdylib), and the
//! generated code compiles against `pyo3` 0.28 with `abi3-py311`.

mod error;
mod function;
mod module;
mod record;
mod ty;

pub use module::render;

/// The rendered output for one interface.
pub struct RenderedInterface {
    /// Sibling items for the exported module: the hidden glue module with
    /// the exception types, `From` impls, `pyfunction` wrappers, record
    /// constructors, and the `pymodule` registration.
    pub glue: proc_macro2::TokenStream,
    /// Attributes to attach to each record struct, index-aligned with the
    /// interface's records.
    pub records: Vec<RenderedRecord>,
}

/// `#[pyclass]`-shaped attributes for one record struct.
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
