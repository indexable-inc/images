//! Render IR types back into Rust token streams for the generated glue.

use proc_macro2::{Ident, Span, TokenStream};
use quote::quote;
use unibind_core::ir;

use crate::RenderError;

/// The Rust spelling of a boundary type, as the wrapper signatures use it.
/// `user` is the exported module's identifier; named types resolve through
/// `super::<user>::`.
pub fn rust_type(ty: &ir::Type, user: &Ident) -> TokenStream {
    match ty {
        ir::Type::Bool => quote!(bool),
        ir::Type::Int(kind) => int_tokens(*kind),
        ir::Type::Float(ir::FloatKind::F32) => quote!(f32),
        ir::Type::Float(ir::FloatKind::F64) => quote!(f64),
        ir::Type::String { owned: true } => quote!(::std::string::String),
        ir::Type::String { owned: false } => quote!(&str),
        ir::Type::Path { owned: true } => quote!(::std::path::PathBuf),
        ir::Type::Path { owned: false } => quote!(&::std::path::Path),
        ir::Type::Bytes { owned: true } => quote!(::std::vec::Vec<u8>),
        ir::Type::Bytes { owned: false } => quote!(&[u8]),
        ir::Type::Option(inner) => {
            let inner = rust_type(inner, user);
            quote!(::std::option::Option<#inner>)
        }
        ir::Type::Vec(inner) => {
            let inner = rust_type(inner, user);
            quote!(::std::vec::Vec<#inner>)
        }
        ir::Type::Map { key, value } => {
            let key = rust_type(key, user);
            let value = rust_type(value, user);
            quote!(::std::collections::HashMap<#key, #value>)
        }
        ir::Type::Named(name) => {
            let name = Ident::new(name, Span::call_site());
            quote!(super::#user::#name)
        }
        ir::Type::Stream(item) => {
            let item = rust_type(item, user);
            quote!(::unibind_runtime::UniStream<#item>)
        }
    }
}

fn int_tokens(kind: ir::IntKind) -> TokenStream {
    match kind {
        ir::IntKind::I8 => quote!(i8),
        ir::IntKind::I16 => quote!(i16),
        ir::IntKind::I32 => quote!(i32),
        ir::IntKind::I64 => quote!(i64),
        ir::IntKind::Isize => quote!(isize),
        ir::IntKind::U8 => quote!(u8),
        ir::IntKind::U16 => quote!(u16),
        ir::IntKind::U32 => quote!(u32),
        ir::IntKind::U64 => quote!(u64),
        ir::IntKind::Usize => quote!(usize),
    }
}

/// The default-value expression for a `#[pyo3(signature = ...)]` entry.
pub fn default_tokens(literal: &ir::Literal) -> TokenStream {
    match literal {
        ir::Literal::Bool(value) => quote!(#value),
        ir::Literal::Int(value) => {
            let value = proc_macro2::Literal::i64_unsuffixed(*value);
            quote!(#value)
        }
        ir::Literal::Float(value) => {
            let value = proc_macro2::Literal::f64_unsuffixed(*value);
            quote!(#value)
        }
        ir::Literal::Str(value) => quote!(#value),
        ir::Literal::None => quote!(None),
    }
}

/// An identifier for a possibly-keyword name (Python renames like `type`
/// fall back to raw identifiers, whose `r#` prefix `pyo3` strips again).
pub fn name_ident(name: &str) -> Result<Ident, RenderError> {
    syn::parse_str::<Ident>(name)
        .or_else(|_| syn::parse_str::<Ident>(&format!("r#{name}")))
        .map_err(|_| RenderError::new(format!("`{name}` is not usable as an identifier")))
}
