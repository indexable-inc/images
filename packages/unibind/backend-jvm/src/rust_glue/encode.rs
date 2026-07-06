//! Encode owned Rust return values into Rust-heap-owned C mirrors.
//!
//! Every allocation made here is reclaimed by the envelope's `Drop` when
//! Java calls the function's `__free` export.

use proc_macro2::TokenStream;
use quote::{format_ident, quote};
use unibind_core::ir;

use crate::model::Model;
use crate::{names, RenderError};

/// The owning helpers the generated encoders call.
pub fn helpers() -> TokenStream {
    quote! {
        /// An absent or empty text value; drops as a no-op.
        fn null_string() -> CString {
            CString {
                ptr: ::core::ptr::null_mut(),
                len: 0,
            }
        }

        /// Leak owned bytes into a mirror; `Drop` reclaims them.
        fn bytes_value(value: ::std::vec::Vec<u8>) -> CBytes {
            let boxed = value.into_boxed_slice();
            let len = boxed.len();
            CBytes {
                ptr: ::std::boxed::Box::into_raw(boxed).cast::<u8>(),
                len,
            }
        }

        /// Leak an owned string into a mirror.
        fn string_value(value: ::std::string::String) -> CString {
            bytes_value(value.into_bytes())
        }

        /// Leak an owned path as UTF-8 text; panics (caught at the export
        /// boundary) on non-UTF-8 paths.
        fn path_value(value: ::std::path::PathBuf) -> CPath {
            let text = value
                .into_os_string()
                .into_string()
                .expect("unibind: path crossing the boundary is not valid UTF-8");
            string_value(text)
        }

        /// Leak an owned vec of mirrors.
        fn vec_value<T>(values: ::std::vec::Vec<T>) -> CVec<T> {
            let boxed = values.into_boxed_slice();
            let len = boxed.len();
            CVec {
                ptr: ::std::boxed::Box::into_raw(boxed).cast::<T>(),
                len,
            }
        }
    }
}

/// The expression encoding the owned Rust value `access` into `ty`'s
/// mirror.
pub fn expr(model: &Model<'_>, ty: &ir::Type, access: &TokenStream) -> Result<TokenStream, RenderError> {
    Ok(match ty {
        ir::Type::Bool => quote!(u8::from(#access)),
        ir::Type::Int(_) | ir::Type::Float(_) => quote!(#access),
        ir::Type::String { .. } => quote!(string_value(#access)),
        ir::Type::Path { .. } => quote!(path_value(#access)),
        ir::Type::Bytes { .. } => quote!(bytes_value(#access)),
        ir::Type::Option(inner) => {
            let value = expr(model, inner, &quote!(value))?;
            quote! {
                match #access {
                    ::std::option::Option::Some(value) => COption {
                        present: 1,
                        value: #value,
                    },
                    ::std::option::Option::None => COption {
                        present: 0,
                        value: unsafe { ::core::mem::zeroed() },
                    },
                }
            }
        }
        ir::Type::Vec(inner) => {
            let element = expr(model, inner, &quote!(element))?;
            quote! {
                vec_value(
                    (#access)
                        .into_iter()
                        .map(|element| #element)
                        .collect::<::std::vec::Vec<_>>(),
                )
            }
        }
        ir::Type::Map { key, value } => {
            let key = expr(model, key, &quote!(key))?;
            let value = expr(model, value, &quote!(value))?;
            quote! {
                vec_value(
                    (#access)
                        .into_iter()
                        .map(|(key, value)| CPair {
                            key: #key,
                            value: #value,
                        })
                        .collect::<::std::vec::Vec<_>>(),
                )
            }
        }
        ir::Type::Named(name) => {
            let mirror = format_ident!("{name}C");
            let mut fields = Vec::new();
            for field in &model.record(name).fields {
                let field_ident = names::rust_ident(&field.name)?;
                let value = expr(model, &field.ty, &quote!(record.#field_ident))?;
                fields.push(quote!(#field_ident: #value));
            }
            quote! {
                {
                    let record = #access;
                    #mirror {
                        #(#fields),*
                    }
                }
            }
        }
    })
}
