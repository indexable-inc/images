//! Shared rendering support for the language backends.
//!
//! Every backend turns an [`ir::Interface`] into the same shape of output
//! (a glue token stream plus per-record attributes), fails the same way (a
//! positioned message), and spells the same Rust tokens for boundary types
//! and identifiers. This module is the single home for that shared
//! surface; language policy (what a backend accepts, how it names
//! host-side artifacts) stays in each backend.

use proc_macro2::{Ident, Span, TokenStream};
use quote::quote;

use crate::ir;

/// The rendered output for one interface: the hidden glue module the
/// backend emits as a sibling of the exported module, plus the attributes
/// it attaches to the user's record structs.
pub struct RenderedInterface {
    /// Sibling items for the exported module: wrappers, error mappings,
    /// and whatever registration the binding library needs.
    pub glue: TokenStream,
    /// Attributes to attach to each record struct, index-aligned with the
    /// interface's records. Backends that read records with plain field
    /// access (jvm) leave every entry empty.
    pub records: Vec<RenderedRecord>,
}

/// Attributes for one record struct, shaped by the backend's binding
/// library (`#[pyclass]`, `#[napi(object)]`, `NifStruct`, or nothing).
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
    pub fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
        }
    }
}

/// An identifier for a possibly-keyword name (renames like `type` fall
/// back to raw identifiers, whose `r#` prefix the binding libraries strip
/// again).
///
/// # Errors
///
/// Fails for a name that is not usable as an identifier even raw.
pub fn name_ident(name: &str) -> Result<Ident, RenderError> {
    syn::parse_str::<Ident>(name)
        .or_else(|_| syn::parse_str::<Ident>(&format!("r#{name}")))
        .map_err(|_| RenderError::new(format!("`{name}` is not usable as an identifier")))
}

/// `snake_case` or `camelCase` to `PascalCase`, for generated class names.
#[must_use]
pub fn pascal_case(name: &str) -> String {
    name.split('_')
        .filter(|segment| !segment.is_empty())
        .map(|segment| {
            let mut chars = segment.chars();
            chars.next().map_or_else(String::new, |first| {
                first.to_ascii_uppercase().to_string() + chars.as_str()
            })
        })
        .collect()
}

/// How to spell borrowed boundary types.
#[derive(Clone, Copy)]
pub enum Ownership {
    /// As the IR declares them (`&str` stays `&str`).
    Declared,
    /// Owned (`&str` becomes `String`): async wrappers move their
    /// arguments into a `'static` future and re-borrow at the call site.
    Owned,
}

/// The Rust spelling of a boundary type, as the wrapper signatures use it.
///
/// `user` is the exported module's identifier; named types resolve through
/// `super::<user>::`. Backends that reject part of this surface (bytes,
/// streams) run their own checks before spelling anything.
#[must_use]
pub fn rust_type(ty: &ir::Type, user: &Ident, ownership: Ownership) -> TokenStream {
    match ty {
        ir::Type::Bool => quote!(bool),
        ir::Type::Int(kind) => int_tokens(*kind),
        ir::Type::Float(ir::FloatKind::F32) => quote!(f32),
        ir::Type::Float(ir::FloatKind::F64) => quote!(f64),
        ir::Type::String { owned } => {
            if matches!((ownership, owned), (Ownership::Declared, false)) {
                quote!(&str)
            } else {
                quote!(::std::string::String)
            }
        }
        ir::Type::Path { owned } => {
            if matches!((ownership, owned), (Ownership::Declared, false)) {
                quote!(&::std::path::Path)
            } else {
                quote!(::std::path::PathBuf)
            }
        }
        ir::Type::Bytes { owned } => {
            if matches!((ownership, owned), (Ownership::Declared, false)) {
                quote!(&[u8])
            } else {
                quote!(::std::vec::Vec<u8>)
            }
        }
        ir::Type::Option(inner) => {
            let inner = rust_type(inner, user, ownership);
            quote!(::std::option::Option<#inner>)
        }
        ir::Type::Vec(inner) => {
            let inner = rust_type(inner, user, ownership);
            quote!(::std::vec::Vec<#inner>)
        }
        ir::Type::Map { key, value } => {
            let key = rust_type(key, user, ownership);
            let value = rust_type(value, user, ownership);
            quote!(::std::collections::HashMap<#key, #value>)
        }
        ir::Type::Named(name) => {
            let name = Ident::new(name, Span::call_site());
            quote!(super::#user::#name)
        }
        ir::Type::Stream(item) => {
            let item = rust_type(item, user, ownership);
            quote!(::unibind_runtime::UniStream<#item>)
        }
    }
}

fn int_tokens(kind: ir::IntKind) -> TokenStream {
    let ident = Ident::new(kind.rust_name(), Span::call_site());
    quote!(#ident)
}
