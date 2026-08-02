//! Attach `#[pyclass]` to record structs and render their constructors.

use proc_macro2::{Ident, Span, TokenStream};
use quote::quote;
use syn::parse_quote;
use unibind_core::ir;
use unibind_core::render::{self, RenderError, RenderedRecord};

use crate::function::doc_attrs;

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

/// A `#[pymethods]` block giving ordinary value records a positional
/// constructor and all-optional option records a keyword-only constructor.
/// Optional fields default to `None`, matching the TypeScript record surface.
pub fn constructor(record: &ir::Record, user: &Ident) -> Result<TokenStream, RenderError> {
    let name = Ident::new(&record.name, Span::call_site());
    let mut params = Vec::new();
    let mut field_idents = Vec::new();
    let mut signature = Vec::new();
    for (index, field) in record.fields.iter().enumerate() {
        let ident = Ident::new(&field.name, Span::call_site());
        let py_ident = render::name_ident(field.names.py.as_ref().unwrap_or(&field.name))?;
        let ty = render::rust_type(&field.ty, user, render::Ownership::Declared);
        params.push(quote!(#py_ident: #ty));
        if matches!(field.ty, ir::Type::Option(_))
            && record.fields[index..]
                .iter()
                .all(|field| matches!(field.ty, ir::Type::Option(_)))
        {
            signature.push(quote!(#py_ident = None));
        } else {
            signature.push(quote!(#py_ident));
        }
        field_idents.push(quote!(#ident: #py_ident));
    }
    let docs = doc_attrs(&record.docs);
    let signature = if !record.fields.is_empty()
        && record
            .fields
            .iter()
            .all(|field| matches!(field.ty, ir::Type::Option(_)))
    {
        quote!(*, #(#signature),*)
    } else {
        quote!(#(#signature),*)
    };
    Ok(quote! {
        #[::pyo3::pymethods]
        impl super::#user::#name {
            #docs
            #[new]
            #[pyo3(signature = (#signature))]
            fn __unibind_new(#(#params),*) -> Self {
                Self {
                    #(#field_idents),*
                }
            }
        }
    })
}
