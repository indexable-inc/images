//! Per-export async-iterator classes for `UniStream` returns.
//!
//! Every stream-returning export gets its own `#[pyclass]`: the item type
//! is baked into the class, so `__anext__` needs no downcasts and Python
//! `isinstance` checks work per export. Which exports those are comes from
//! `unibind_core::render::stream_exports`, shared with the other backends
//! that render stream methods; the naming and the class body are this
//! backend's. `unibind-gen`'s `.pyi` emitter consumes them too: the stub
//! declares exactly the classes the glue registers.

use proc_macro2::{Ident, TokenStream};
use quote::{format_ident, quote};
use unibind_core::render::{self, StreamExport, pascal_case};

use crate::ctx::Ctx;

/// The `#[pyclass]` for one stream-returning export.
#[must_use]
pub fn render(export: &StreamExport<'_>, ctx: &Ctx<'_>) -> TokenStream {
    let ident = class_ident(export.owner, &export.function.name);
    let py_name = class_name(export.owner, &export.function.name);
    let item_ty = render::rust_type(export.item, ctx.user, render::Ownership::Declared);
    let doc_source = format!("Async iterator produced by `{}`.", export.qualified_name());
    let doc_pull = "Pull-based: each `__anext__` polls exactly one item, so the \
                    producer only runs as fast as the consumer awaits.";
    quote! {
        #[doc = #doc_source]
        #[doc = ""]
        #[doc = #doc_pull]
        #[::pyo3::pyclass(name = #py_name, frozen)]
        struct #ident {
            stream: ::unibind_py_runtime::SharedStream<#item_ty>,
        }
        impl #ident {
            fn __unibind_wrap(stream: ::unibind_runtime::UniStream<#item_ty>) -> Self {
                Self {
                    stream: ::unibind_py_runtime::SharedStream::new(stream),
                }
            }
        }
        #[::pyo3::pymethods]
        impl #ident {
            fn __aiter__(slf: ::pyo3::PyRef<'_, Self>) -> ::pyo3::PyRef<'_, Self> {
                slf
            }
            fn __anext__<'py>(
                &self,
                py: ::pyo3::Python<'py>,
            ) -> ::pyo3::PyResult<::pyo3::Bound<'py, ::pyo3::PyAny>> {
                self.stream.next_into_py(py)
            }
        }
    }
}

/// The Rust identifier of a stream class.
///
/// Export names are unique per scope (Rust enforces it), so classes cannot
/// collide within a scope; a free function named exactly like an
/// object+method concatenation would collide across scopes, and fails loudly
/// as a duplicate item in the glue module rather than silently misbinding.
#[must_use]
pub fn class_ident(owner: Option<&str>, export: &str) -> Ident {
    let export = pascal_case(export);
    owner.map_or_else(
        || format_ident!("UnibindStream{export}"),
        |object| format_ident!("UnibindStream{object}{export}"),
    )
}

/// The Python-visible class name: `TailStream` for a free `tail`,
/// `StoreWatchStream` for `Store::watch`. Built from the Rust names;
/// renames never reach these classes.
#[must_use]
pub fn class_name(owner: Option<&str>, export: &str) -> String {
    let export = pascal_case(export);
    owner.map_or_else(
        || format!("{export}Stream"),
        |object| format!("{object}{export}Stream"),
    )
}
