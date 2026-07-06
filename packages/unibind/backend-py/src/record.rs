//! Attach `#[pyclass]` to record structs and render their constructors.

use proc_macro2::{Ident, Span, TokenStream};
use quote::quote;
use syn::parse_quote;
use unibind_core::ir;

use crate::function::doc_attrs;
use crate::{ty, RenderError, RenderedRecord};

/// The attributes the exported struct gains: `#[pyclass]` on the item and a
/// read-only getter per field.
pub fn record_attrs(record: &ir::Record) -> RenderedRecord {
    let outer: syn::Attribute = record.names.py.as_ref().map_or_else(
        || parse_quote!(#[::pyo3::pyclass(from_py_object)]),
        |name| parse_quote!(#[::pyo3::pyclass(from_py_object, name = #name)]),
    );
    let fields = record
        .fields
        .iter()
        .map(|field| {
            let attr: syn::Attribute = field.names.py.as_ref().map_or_else(
                || parse_quote!(#[pyo3(get)]),
                |name| parse_quote!(#[pyo3(get, name = #name)]),
            );
            vec![attr]
        })
        .collect();
    RenderedRecord {
        outer: vec![outer],
        fields,
    }
}

/// A `#[pymethods]` block giving the record a positional-or-keyword
/// constructor, so Python can build values as well as receive them.
pub fn constructor(record: &ir::Record, user: &Ident) -> Result<TokenStream, RenderError> {
    let name = Ident::new(&record.name, Span::call_site());
    let mut params = Vec::new();
    let mut field_idents = Vec::new();
    let mut signature = Vec::new();
    for field in &record.fields {
        let ident = Ident::new(&field.name, Span::call_site());
        let py_ident = ty::name_ident(field.names.py.as_ref().unwrap_or(&field.name))?;
        let ty = ty::rust_type(&field.ty, user);
        params.push(quote!(#py_ident: #ty));
        signature.push(quote!(#py_ident));
        field_idents.push(quote!(#ident: #py_ident));
    }
    let docs = doc_attrs(&record.docs);
    Ok(quote! {
        #[::pyo3::pymethods]
        impl super::#user::#name {
            #docs
            #[new]
            #[pyo3(signature = (#(#signature),*))]
            fn __unibind_new(#(#params),*) -> Self {
                Self {
                    #(#field_idents),*
                }
            }
        }
    })
}
