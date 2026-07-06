//! The `#[repr(C)]` mirror structs the glue module carries.
//!
//! Shapes here must stay in lockstep with the layout math in
//! [`crate::ctype`]; the assertions in [`crate::rust_glue::asserts`] make
//! the compiler prove it.

use proc_macro2::{Ident, TokenStream};
use quote::{format_ident, quote};
use unibind_core::ir;

use crate::ctype::CTy;
use crate::{names, RenderError};

/// The fixed mirror types every glue module defines. Drops are null-safe on
/// purpose: an all-zero mirror (an absent `COption`, an error-path `value`)
/// drops as a no-op, so envelopes free unconditionally.
pub fn runtime() -> TokenStream {
    quote! {
        /// UTF-8 text as pointer + length. Argument values are Java-owned
        /// and only viewed; returned values leak a boxed slice that the
        /// envelope's `Drop` reclaims.
        #[repr(C)]
        pub struct CString {
            pub ptr: *mut u8,
            pub len: usize,
        }

        impl ::core::ops::Drop for CString {
            fn drop(&mut self) {
                if self.ptr.is_null() {
                    return;
                }
                drop(unsafe {
                    ::std::boxed::Box::from_raw(::core::ptr::slice_from_raw_parts_mut(
                        self.ptr, self.len,
                    ))
                });
            }
        }

        /// Raw bytes cross exactly like text.
        pub type CBytes = CString;
        /// Paths cross as UTF-8 text.
        pub type CPath = CString;

        /// A boxed slice as pointer + length.
        #[repr(C)]
        pub struct CVec<T> {
            pub ptr: *mut T,
            pub len: usize,
        }

        impl<T> ::core::ops::Drop for CVec<T> {
            fn drop(&mut self) {
                if self.ptr.is_null() {
                    return;
                }
                drop(unsafe {
                    ::std::boxed::Box::from_raw(::core::ptr::slice_from_raw_parts_mut(
                        self.ptr, self.len,
                    ))
                });
            }
        }

        /// Inline optional: absent means `present == 0` with `value` all
        /// zeroed.
        #[repr(C)]
        pub struct COption<T> {
            pub present: u8,
            pub value: T,
        }

        /// One map entry; a map crosses as `CVec<CPair<K, V>>`.
        #[repr(C)]
        pub struct CPair<K, V> {
            pub key: K,
            pub value: V,
        }
    }
}

/// The Rust spelling of one mirror inside the glue module.
pub fn mirror_tokens(ty: &CTy) -> TokenStream {
    match ty {
        CTy::Bool => quote!(u8),
        CTy::Int(kind) => int_tokens(*kind),
        CTy::Float(ir::FloatKind::F32) => quote!(f32),
        CTy::Float(ir::FloatKind::F64) => quote!(f64),
        CTy::Str => quote!(CString),
        CTy::Path => quote!(CPath),
        CTy::Bytes => quote!(CBytes),
        CTy::Option(inner) => {
            let inner = mirror_tokens(inner);
            quote!(COption<#inner>)
        }
        CTy::Vec(inner) => {
            let inner = mirror_tokens(inner);
            quote!(CVec<#inner>)
        }
        CTy::Map { key, value } => {
            let key = mirror_tokens(key);
            let value = mirror_tokens(value);
            quote!(CVec<CPair<#key, #value>>)
        }
        CTy::Record(name) => {
            let ident = format_ident!("{name}C");
            quote!(#ident)
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

/// One `#[repr(C)]` mirror struct per record, fields in declaration order.
pub fn record_mirrors(interface: &ir::Interface) -> Result<TokenStream, RenderError> {
    let mut out = TokenStream::new();
    for record in &interface.records {
        let ident = format_ident!("{}C", record.name);
        let doc = format!("C mirror of `{}`, fields in declaration order.", record.name);
        let mut fields = Vec::new();
        for field in &record.fields {
            let name = names::rust_ident(&field.name)?;
            let ty = mirror_tokens(&CTy::of(&field.ty));
            fields.push(quote!(pub #name: #ty));
        }
        out.extend(quote! {
            #[doc = #doc]
            #[repr(C)]
            pub struct #ident {
                #(#fields,)*
            }
        });
    }
    Ok(out)
}

/// One return envelope struct per function.
pub fn envelopes(interface: &ir::Interface) -> TokenStream {
    let mut out = TokenStream::new();
    for function in &interface.functions {
        let ident = envelope_ident(&function.name);
        let doc = format!(
            "Return envelope for `{}`: `code` 0 ok, N the N-th `throws` variant, -1 panic.",
            function.name
        );
        let value = function.ret.as_ref().map(|ret| {
            let ty = mirror_tokens(&CTy::of(ret));
            quote!(pub value: #ty,)
        });
        out.extend(quote! {
            #[doc = #doc]
            #[repr(C)]
            pub struct #ident {
                pub code: i32,
                pub err_msg: CString,
                #value
            }
        });
    }
    out
}

/// The envelope struct identifier for one function.
pub fn envelope_ident(function: &str) -> Ident {
    format_ident!("{}Envelope", names::pascal(function))
}
