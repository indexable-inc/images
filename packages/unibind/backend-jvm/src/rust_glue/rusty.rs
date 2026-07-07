//! Render owned IR types back into Rust token streams.
//!
//! Mirrors `backend-py`'s `ty.rs`: the glue needs the user-facing Rust
//! spelling for stream item turbofish (`stream_next::<T>`) and inside the
//! spawned async bodies, resolving `Named` types through
//! `super::<user>::`. Only owned spellings ever appear in those positions
//! (stream items and async arguments are owned by lowering), but the
//! borrowed spellings render too so the mapping stays total.

use proc_macro2::{Ident, TokenStream};
use quote::{format_ident, quote};
use unibind_core::ir;

/// The Rust spelling of one boundary type inside the glue module.
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
            let name = format_ident!("{name}");
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
