//! Decode C mirrors into the Rust values the user's function takes.
//!
//! Arguments are owned by the Java arena: borrowed parameter types (`&str`,
//! `&Path`, `&[u8]`) view the Java memory zero-copy, owned ones copy out of
//! it, and nothing here ever frees it.

use proc_macro2::{Ident, TokenStream};
use quote::quote;
use unibind_core::ir;

use crate::model::Model;
use crate::{names, RenderError};

/// The borrow helpers the generated decoders call.
pub fn helpers() -> TokenStream {
    quote! {
        /// View a Java-owned buffer; empty buffers may carry a null pointer.
        unsafe fn view<'a, T>(ptr: *const T, len: usize) -> &'a [T] {
            if len == 0 {
                &[]
            } else {
                unsafe { ::core::slice::from_raw_parts(ptr, len) }
            }
        }

        /// Borrow Java-owned text; panics (caught at the export boundary)
        /// on invalid UTF-8.
        unsafe fn str_value(value: &CString) -> &str {
            ::core::str::from_utf8(unsafe { view(value.ptr, value.len) })
                .expect("unibind: text crossing the boundary is not valid UTF-8")
        }
    }
}

/// The expression decoding `access` into `ty`. `access` is a mirror value
/// for scalars and a mirror reference for aggregates; every generated
/// `unsafe` operation carries its own block, so the result is usable in
/// safe positions.
pub fn expr(
    model: &Model<'_>,
    ty: &ir::Type,
    access: &TokenStream,
    user: &Ident,
) -> Result<TokenStream, RenderError> {
    Ok(match ty {
        ir::Type::Bool => quote!((#access != 0)),
        ir::Type::Int(_) | ir::Type::Float(_) => quote!(#access),
        ir::Type::String { owned: false } => quote!(unsafe { str_value(#access) }),
        ir::Type::String { owned: true } => {
            quote!(::std::borrow::ToOwned::to_owned(unsafe { str_value(#access) }))
        }
        ir::Type::Path { owned: false } => {
            quote!(::std::path::Path::new(unsafe { str_value(#access) }))
        }
        ir::Type::Path { owned: true } => {
            quote!(::std::path::PathBuf::from(unsafe { str_value(#access) }))
        }
        ir::Type::Bytes { owned: false } => {
            quote!(unsafe { view((#access).ptr, (#access).len) })
        }
        ir::Type::Bytes { owned: true } => {
            quote!(unsafe { view((#access).ptr, (#access).len) }.to_vec())
        }
        ir::Type::Option(inner) => {
            let value_place = place(access, &quote!(value), inner);
            let value = expr(model, inner, &value_place, user)?;
            quote! {
                if (#access).present != 0 {
                    ::std::option::Option::Some(#value)
                } else {
                    ::std::option::Option::None
                }
            }
        }
        ir::Type::Vec(inner) => {
            let element_place = element_place(inner);
            let element = expr(model, inner, &element_place, user)?;
            quote! {
                unsafe { view((#access).ptr, (#access).len) }
                    .iter()
                    .map(|element| #element)
                    .collect::<::std::vec::Vec<_>>()
            }
        }
        ir::Type::Map { key, value } => {
            let key_place = place(&quote!(entry), &quote!(key), key);
            let value_place = place(&quote!(entry), &quote!(value), value);
            let key = expr(model, key, &key_place, user)?;
            let value = expr(model, value, &value_place, user)?;
            quote! {
                unsafe { view((#access).ptr, (#access).len) }
                    .iter()
                    .map(|entry| (#key, #value))
                    .collect::<::std::collections::HashMap<_, _>>()
            }
        }
        ir::Type::Stream(_) => {
            return Err(RenderError::new(
                "`UniStream` crosses as an async iterator; the JVM backend's stream \
                 support lands with issue #2083",
            ));
        }
        ir::Type::Named(name) => {
            let ident = names::rust_ident(name)?;
            let mut fields = Vec::new();
            for field in &model.record(name).fields {
                let field_ident = names::rust_ident(&field.name)?;
                let field_place = place(access, &quote!(#field_ident), &field.ty);
                let value = expr(model, &field.ty, &field_place, user)?;
                fields.push(quote!(#field_ident: #value));
            }
            quote!(super::#user::#ident { #(#fields),* })
        }
    })
}

/// The place expression for `parent.field`: value form for scalars,
/// reference form for aggregates.
fn place(parent: &TokenStream, field: &TokenStream, ty: &ir::Type) -> TokenStream {
    if is_scalar(ty) {
        quote!(((#parent).#field))
    } else {
        quote!((&(#parent).#field))
    }
}

/// The place expression for one `&mirror` iterator element.
fn element_place(ty: &ir::Type) -> TokenStream {
    if is_scalar(ty) {
        quote!((*element))
    } else {
        quote!(element)
    }
}

const fn is_scalar(ty: &ir::Type) -> bool {
    matches!(ty, ir::Type::Bool | ir::Type::Int(_) | ir::Type::Float(_))
}
