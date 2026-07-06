//! Render IR types back into Rust token streams for the generated glue,
//! and validate that a type is representable on the BEAM boundary.

use proc_macro2::{Ident, Span, TokenStream};
use quote::quote;
use unibind_core::ir;

use crate::RenderError;

/// The Rust spelling of a boundary type, as the wrapper signatures use it.
/// `user` is the exported module's identifier; named types resolve through
/// `super::<user>::`.
///
/// Rejects the types rustler cannot carry: see [`check_boundary`].
pub fn rust_type(ty: &ir::Type, user: &Ident) -> Result<TokenStream, RenderError> {
    check_boundary(ty)?;
    Ok(spell(ty, user, Ownership::Declared))
}

/// Like [`rust_type`], but with every borrow owned: async wrappers move
/// their arguments into a `'static` future, so `&str` arrives as `String`
/// and is re-borrowed at the call site.
pub fn owned_type(ty: &ir::Type, user: &Ident) -> Result<TokenStream, RenderError> {
    check_boundary(ty)?;
    Ok(spell(ty, user, Ownership::Owned))
}

/// How to spell borrowed boundary types.
#[derive(Clone, Copy)]
enum Ownership {
    /// As the IR declares them (`&str` stays `&str`).
    Declared,
    /// Owned (`&str` becomes `String`).
    Owned,
}

fn spell(ty: &ir::Type, user: &Ident, ownership: Ownership) -> TokenStream {
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
        ir::Type::Option(inner) => {
            let inner = spell(inner, user, ownership);
            quote!(::std::option::Option<#inner>)
        }
        ir::Type::Vec(inner) => {
            let inner = spell(inner, user, ownership);
            quote!(::std::vec::Vec<#inner>)
        }
        ir::Type::Map { key, value } => {
            let key = spell(key, user, ownership);
            let value = spell(value, user, ownership);
            quote!(::std::collections::HashMap<#key, #value>)
        }
        ir::Type::Named(name) => {
            let name = Ident::new(name, Span::call_site());
            quote!(super::#user::#name)
        }
        // Unrepresentable variants are rejected by `check_boundary` before
        // anything is spelled.
        ir::Type::Bytes { .. } | ir::Type::Stream(_) => {
            unreachable!("rejected by check_boundary")
        }
    }
}

/// The call-site expression forwarding an owned wrapper argument to a user
/// function that may expect the declared borrow.
pub fn reborrow(name: &Ident, ty: &ir::Type) -> TokenStream {
    match ty {
        ir::Type::String { owned: false } | ir::Type::Path { owned: false } => quote!(&#name),
        ir::Type::Option(inner)
            if matches!(
                **inner,
                ir::Type::String { owned: false } | ir::Type::Path { owned: false }
            ) =>
        {
            quote!(#name.as_deref())
        }
        _ => quote!(#name),
    }
}

/// Reject the types the elixir backend cannot carry across the boundary:
/// binaries (no `Vec<u8>` <-> Elixir binary codec in rustler; land with
/// stage 2) and nested streams (`Stream<T>` only crosses as a whole
/// function return).
pub fn check_boundary(ty: &ir::Type) -> Result<(), RenderError> {
    match ty {
        ir::Type::Bytes { .. } => Err(RenderError::new(
            "binary payloads (`Vec<u8>` / `&[u8]`) are not part of the \
             elixir backend yet; carry the bytes as `String` for now",
        )),
        ir::Type::Stream(_) => Err(RenderError::new(
            "`Stream<T>` only crosses as the whole return type of a stream \
             function",
        )),
        ir::Type::Option(inner) | ir::Type::Vec(inner) => check_boundary(inner),
        ir::Type::Map { key, value } => {
            check_boundary(key)?;
            check_boundary(value)
        }
        ir::Type::Bool
        | ir::Type::Int(_)
        | ir::Type::Float(_)
        | ir::Type::String { .. }
        | ir::Type::Path { .. }
        | ir::Type::Named(_) => Ok(()),
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
