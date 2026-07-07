//! The `#[repr(C)]` mirror structs the glue module carries.
//!
//! Shapes here must stay in lockstep with the layout math in
//! [`crate::ctype`]; the assertions in [`crate::rust_glue::asserts`] make
//! the compiler prove it.

use proc_macro2::{Ident, TokenStream};
use quote::{format_ident, quote};
use unibind_core::ir;

use crate::ctype::CTy;
use crate::model::Model;
use crate::rust_glue::Export;
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

        /// A raw pointer moved into a spawned task's completion closure.
        /// The Java side routes every callback through a static upcall
        /// stub plus a registry id, so carrying the pointer across threads
        /// is sound by that contract.
        pub struct SendPtr(pub *mut ::core::ffi::c_void);

        unsafe impl ::core::marker::Send for SendPtr {}
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
        CTy::Handle => quote!(*mut ::core::ffi::c_void),
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

/// One return envelope struct per export (free function, constructor, or
/// method), plus one item envelope per stream-returning export.
pub fn envelopes(model: &Model<'_>, exports: &[Export<'_>]) -> TokenStream {
    let mut out = TokenStream::new();
    for export in exports {
        let ident = envelope_ident(export.owner, &export.function.name);
        let doc = format!(
            "Return envelope for `{}`: `code` 0 ok, N the N-th `throws` variant, -1 panic, \
             -2 cancelled (async exports only).",
            export.site()
        );
        let value = export.ret.as_ref().map(|ret| {
            let ty = mirror_tokens(&model.boundary(ret));
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
        if let Some(ir::Type::Stream(item)) = &export.ret {
            let item_ident = item_envelope_ident(export.owner, &export.function.name);
            let item_doc = format!(
                "Item envelope for `{}`: `code` 0 with a present value is an item, 0 with an \
                 absent value is end-of-stream, -1 is a producer panic.",
                export.site()
            );
            let value_ty = mirror_tokens(&CTy::Option(Box::new(CTy::of(item))));
            out.extend(quote! {
                #[doc = #item_doc]
                #[repr(C)]
                pub struct #item_ident {
                    pub code: i32,
                    pub err_msg: CString,
                    pub value: #value_ty,
                }
            });
        }
    }
    out
}

/// The envelope struct identifier for one export; methods and constructors
/// scope theirs by the owning object's name.
pub fn envelope_ident(owner: Option<&str>, function: &str) -> Ident {
    match owner {
        Some(object) => format_ident!("{object}{}Envelope", names::pascal(function)),
        None => format_ident!("{}Envelope", names::pascal(function)),
    }
}

/// The item envelope struct identifier for one stream export.
pub fn item_envelope_ident(owner: Option<&str>, function: &str) -> Ident {
    match owner {
        Some(object) => format_ident!("{object}{}ItemEnvelope", names::pascal(function)),
        None => format_ident!("{}ItemEnvelope", names::pascal(function)),
    }
}
