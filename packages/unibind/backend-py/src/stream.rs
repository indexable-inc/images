//! Per-export async-iterator classes for `UniStream` returns.
//!
//! Every stream-returning export gets its own `#[pyclass]`: the item type
//! is baked into the class, so `__anext__` needs no downcasts and Python
//! `isinstance` checks work per export.

use proc_macro2::{Ident, TokenStream};
use quote::{format_ident, quote};
use unibind_core::ir;

use crate::ctx::Ctx;
use crate::ty;

/// One stream-returning export: the callable that produced it plus the
/// item type its class yields.
pub struct StreamExport<'a> {
    /// `None` for free functions, the owning object's name for methods.
    owner: Option<&'a str>,
    function: &'a ir::Function,
    item: &'a ir::Type,
}

/// Every stream-returning export in the interface, in render order (free
/// functions first, then each object's methods).
pub fn collect(interface: &ir::Interface) -> Vec<StreamExport<'_>> {
    let free = interface
        .functions
        .iter()
        .filter_map(|function| stream_export(None, function));
    let methods = interface.objects.iter().flat_map(|object| {
        object
            .methods
            .iter()
            .filter_map(|method| stream_export(Some(object.name.as_str()), method))
    });
    free.chain(methods).collect()
}

fn stream_export<'a>(
    owner: Option<&'a str>,
    function: &'a ir::Function,
) -> Option<StreamExport<'a>> {
    let Some(ir::Type::Stream(item)) = &function.ret else {
        return None;
    };
    Some(StreamExport {
        owner,
        function,
        item,
    })
}

impl StreamExport<'_> {
    pub fn class_ident(&self) -> Ident {
        class_ident(self.owner, &self.function.name)
    }

    pub fn render(&self, ctx: &Ctx<'_>) -> TokenStream {
        let ident = self.class_ident();
        let py_name = python_name(self.owner, &self.function.name);
        let item_ty = ty::rust_type(self.item, ctx.user);
        let produced = self.owner.map_or_else(
            || self.function.name.clone(),
            |object| format!("{object}.{}", self.function.name),
        );
        let doc_source = format!("Async iterator produced by `{produced}`.");
        let doc_pull = "Pull-based: each `__anext__` polls exactly one item, so the \
                        producer only runs as fast as the consumer awaits.";
        quote! {
            #[doc = #doc_source]
            #[doc = ""]
            #[doc = #doc_pull]
            #[::pyo3::pyclass(name = #py_name, frozen)]
            struct #ident {
                stream: ::unibind_runtime::py::SharedStream<#item_ty>,
            }
            impl #ident {
                fn __unibind_wrap(stream: ::unibind_runtime::UniStream<#item_ty>) -> Self {
                    Self {
                        stream: ::unibind_runtime::py::SharedStream::new(stream),
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
                    let next = self.stream.next();
                    ::unibind_runtime::py::future_into_py(py, async move {
                        match next.await {
                            ::std::option::Option::Some(item) => ::pyo3::PyResult::Ok(item),
                            ::std::option::Option::None => ::pyo3::PyResult::Err(
                                ::pyo3::exceptions::PyStopAsyncIteration::new_err(()),
                            ),
                        }
                    })
                }
            }
        }
    }
}

/// The Rust identifier of a stream class. Export names are unique per
/// scope (Rust enforces it), so classes cannot collide within a scope; a
/// free function named exactly like an object+method concatenation would
/// collide across scopes, and fails loudly as a duplicate item in the glue
/// module rather than silently misbinding.
pub fn class_ident(owner: Option<&str>, export: &str) -> Ident {
    let export = pascal_case(export);
    owner.map_or_else(
        || format_ident!("UnibindStream{export}"),
        |object| format_ident!("UnibindStream{object}{export}"),
    )
}

/// The Python-visible class name: `TailStream` / `StoreRowsStream`.
fn python_name(owner: Option<&str>, export: &str) -> String {
    let export = pascal_case(export);
    owner.map_or_else(
        || format!("{export}Stream"),
        |object| format!("{object}{export}Stream"),
    )
}

/// `snake_case` -> `PascalCase` for export names.
fn pascal_case(name: &str) -> String {
    name.split('_')
        .map(|segment| {
            let mut chars = segment.chars();
            chars.next().map_or_else(String::new, |first| {
                first.to_ascii_uppercase().to_string() + chars.as_str()
            })
        })
        .collect()
}
