//! The shared type mapping: IR types to stable (ABI) and idiomatic Rust
//! spellings, plus the conversion expressions between them.
//!
//! Both render surfaces go through this one module — the engine glue and the
//! generated client must agree on every boundary type byte-for-byte, so the
//! mapping exists exactly once and drift is structurally impossible.

use proc_macro2::{Ident, Span, TokenStream};
use quote::quote;
use unibind_core::ir;

/// Where named (record / error) types resolve on the non-ABI side, and where
/// the mirror types resolve. The engine glue sees the user's structs through
/// `super::<module>::` and its mirrors as bare siblings; the client sees its
/// idiomatic records through `crate::records::` and mirrors through
/// `crate::abi::`.
#[derive(Debug, Clone)]
pub struct Paths {
    /// Prefix for the plain (user / idiomatic) side of a conversion.
    pub plain: TokenStream,
    /// Prefix for the ABI-stable mirror side.
    pub mirror: TokenStream,
    /// The `#[stabby::stabby(module = ...)]` override stamped on every
    /// generated mirror. stabby's report check compares module paths, and
    /// the engine's and client's mirrors live in different crates, so both
    /// sides pin the same logical namespace (`unibind::<interface>`)
    /// instead of their real `module_path!()`.
    pub report_module: String,
}

/// The stable (ABI) spelling of a boundary type. Borrowed strings, paths,
/// and byte slices cross by value: the boundary conversion clones anyway, so
/// one owned representation serves both.
pub fn stable_type(ty: &ir::Type, paths: &Paths) -> TokenStream {
    match ty {
        ir::Type::Bool => quote!(bool),
        ir::Type::Int(kind) => int_tokens(*kind),
        ir::Type::Float(ir::FloatKind::F32) => quote!(f32),
        ir::Type::Float(ir::FloatKind::F64) => quote!(f64),
        ir::Type::String { .. } => quote!(::stabby::string::String),
        // Unix `OsStr` bytes: lossless, unlike a UTF-8 round trip.
        ir::Type::Path { .. } | ir::Type::Bytes { .. } => quote!(::stabby::vec::Vec<u8>),
        ir::Type::Option(inner) => {
            let inner = stable_type(inner, paths);
            quote!(::stabby::option::Option<#inner>)
        }
        ir::Type::Vec(inner) => {
            let inner = stable_type(inner, paths);
            quote!(::stabby::vec::Vec<#inner>)
        }
        // stabby has no stable HashMap (ZettaScaleLabs/stabby#109), so a map
        // crosses as a vec of pairs and reassembles on the other side.
        ir::Type::Map { key, value } => {
            let key = stable_type(key, paths);
            let value = stable_type(value, paths);
            quote!(::stabby::vec::Vec<::stabby::tuple::Tuple2<#key, #value>>)
        }
        // A `UniStream<T>` return crosses as a shared-crate `dynptr` box:
        // the protocol trait must live in one crate both sides link (see
        // `unibind-stream`).
        ir::Type::Stream(item) => {
            let item = stable_type(item, paths);
            quote!(::unibind_stream::DynStream<'static, #item>)
        }
        ir::Type::Named(name) => {
            let name = name_ident(name);
            let mirror = &paths.mirror;
            quote!(#mirror #name)
        }
    }
}

/// The idiomatic (std) spelling, as the client's safe surface and the user's
/// exported functions use it. `borrowed` spellings only appear in argument
/// position, mirroring the lowering rules.
pub fn plain_type(ty: &ir::Type, paths: &Paths) -> TokenStream {
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
            let inner = plain_type(inner, paths);
            quote!(::std::option::Option<#inner>)
        }
        ir::Type::Vec(inner) => {
            let inner = plain_type(inner, paths);
            quote!(::std::vec::Vec<#inner>)
        }
        ir::Type::Map { key, value } => {
            let key = plain_type(key, paths);
            let value = plain_type(value, paths);
            quote!(::std::collections::HashMap<#key, #value>)
        }
        // Streams never spell plainly: methods return a generated named
        // wrapper, handled where functions render.
        ir::Type::Stream(_) => unreachable!("streams only exist at return position"),
        ir::Type::Named(name) => {
            let name = name_ident(name);
            let plain = &paths.plain;
            quote!(#plain #name)
        }
    }
}

/// Whether the plain and stable spellings coincide, making the conversion
/// the value itself (primitives cross as-is).
pub const fn is_identity(ty: &ir::Type) -> bool {
    matches!(ty, ir::Type::Bool | ir::Type::Int(_) | ir::Type::Float(_))
}

/// Convert `expr` (a plain value, possibly borrowed) into its stable
/// representation.
pub fn to_stable(expr: &TokenStream, ty: &ir::Type, paths: &Paths) -> TokenStream {
    match ty {
        ir::Type::Bool | ir::Type::Int(_) | ir::Type::Float(_) => expr.clone(),
        ir::Type::String { .. } => quote!(::stabby::string::String::from(#expr)),
        ir::Type::Path { owned: true } => quote! {
            ::std::os::unix::ffi::OsStringExt::into_vec(#expr.into_os_string())
                .into_iter()
                .collect::<::stabby::vec::Vec<u8>>()
        },
        ir::Type::Path { owned: false } => quote! {
            ::stabby::vec::Vec::from(::std::os::unix::ffi::OsStrExt::as_bytes(#expr.as_os_str()))
        },
        ir::Type::Bytes { owned: true } => {
            quote!(#expr.into_iter().collect::<::stabby::vec::Vec<u8>>())
        }
        ir::Type::Bytes { owned: false } => quote!(::stabby::vec::Vec::from(#expr)),
        ir::Type::Option(inner) => {
            // `map_or_else`, not a match: the generated code compiles under
            // the workspace's nursery gate (`clippy::option_if_let_else`).
            let converted = to_stable(&quote!(inner), inner, paths);
            quote! {
                #expr.map_or_else(
                    ::stabby::option::Option::None,
                    |inner| ::stabby::option::Option::Some(#converted),
                )
            }
        }
        ir::Type::Vec(inner) => {
            let target = stable_type(ty, paths);
            if is_identity(inner) {
                // No `.map(|item| item)`: the element crosses as-is.
                return quote!(#expr.into_iter().collect::<#target>());
            }
            let converted = to_stable(&quote!(item), inner, paths);
            quote!(#expr.into_iter().map(|item| #converted).collect::<#target>())
        }
        ir::Type::Map { key, value } => {
            let target = stable_type(ty, paths);
            let key_converted = to_stable(&quote!(key), key, paths);
            let value_converted = to_stable(&quote!(value), value, paths);
            quote! {
                #expr
                    .into_iter()
                    .map(|(key, value)| ::stabby::tuple::Tuple2::from((#key_converted, #value_converted)))
                    .collect::<#target>()
            }
        }
        ir::Type::Stream(_) => unreachable!("streams only exist at return position"),
        ir::Type::Named(name) => {
            let name = name_ident(name);
            let mirror = &paths.mirror;
            quote!(#mirror #name::from(#expr))
        }
    }
}

/// Convert `expr` (a stable value) back into its plain, owned
/// representation. Borrowed types come back owned; the caller borrows at the
/// call site.
pub fn to_plain(expr: &TokenStream, ty: &ir::Type, paths: &Paths) -> TokenStream {
    match ty {
        ir::Type::Bool | ir::Type::Int(_) | ir::Type::Float(_) => expr.clone(),
        ir::Type::String { .. } => quote!(::std::string::String::from(#expr)),
        ir::Type::Path { .. } => quote! {
            ::std::path::PathBuf::from(::std::os::unix::ffi::OsStringExt::from_vec(
                #expr.into_iter().collect::<::std::vec::Vec<u8>>(),
            ))
        },
        ir::Type::Bytes { .. } => quote!(#expr.into_iter().collect::<::std::vec::Vec<u8>>()),
        ir::Type::Option(inner) => {
            let converted = to_plain(&quote!(inner), inner, paths);
            quote! {
                #expr.match_owned(
                    |inner| ::std::option::Option::Some(#converted),
                    || ::std::option::Option::None,
                )
            }
        }
        ir::Type::Vec(inner) => {
            let target = owned_plain_type(ty, paths);
            if is_identity(inner) {
                // No `.map(|item| item)`: the element crosses as-is.
                return quote!(#expr.into_iter().collect::<#target>());
            }
            let converted = to_plain(&quote!(item), inner, paths);
            quote!(#expr.into_iter().map(|item| #converted).collect::<#target>())
        }
        ir::Type::Map { key, value } => {
            let target = owned_plain_type(ty, paths);
            let key_stable = stable_type(key, paths);
            let value_stable = stable_type(value, paths);
            let key_converted = to_plain(&quote!(key), key, paths);
            let value_converted = to_plain(&quote!(value), value, paths);
            quote! {
                #expr
                    .into_iter()
                    .map(|pair| {
                        let (key, value): (#key_stable, #value_stable) = pair.into();
                        (#key_converted, #value_converted)
                    })
                    .collect::<#target>()
            }
        }
        ir::Type::Stream(_) => unreachable!("streams only exist at return position"),
        ir::Type::Named(name) => {
            let name = name_ident(name);
            let plain = &paths.plain;
            quote!(#plain #name::from(#expr))
        }
    }
}

/// The owned spelling of a plain type, for conversion targets: borrowed
/// argument types (`&str`, `&Path`, `&[u8]`) come back as their owned
/// counterparts.
pub fn owned_plain_type(ty: &ir::Type, paths: &Paths) -> TokenStream {
    match ty {
        ir::Type::String { .. } => quote!(::std::string::String),
        ir::Type::Path { .. } => quote!(::std::path::PathBuf),
        ir::Type::Bytes { .. } => quote!(::std::vec::Vec<u8>),
        ir::Type::Option(inner) => {
            let inner = owned_plain_type(inner, paths);
            quote!(::std::option::Option<#inner>)
        }
        ir::Type::Vec(inner) => {
            let inner = owned_plain_type(inner, paths);
            quote!(::std::vec::Vec<#inner>)
        }
        ir::Type::Map { key, value } => {
            let key = owned_plain_type(key, paths);
            let value = owned_plain_type(value, paths);
            quote!(::std::collections::HashMap<#key, #value>)
        }
        _ => plain_type(ty, paths),
    }
}

/// Whether a plain value of `ty` is borrowed at the user's call surface, so
/// conversions bind an owned local and pass `&local`.
pub const fn is_borrowed(ty: &ir::Type) -> bool {
    matches!(
        ty,
        ir::Type::String { owned: false }
            | ir::Type::Path { owned: false }
            | ir::Type::Bytes { owned: false }
    )
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

/// An identifier for an IR-provided name. IR names come from parsed Rust
/// identifiers, so this cannot fail for lowered interfaces.
pub fn name_ident(name: &str) -> Ident {
    Ident::new(name, Span::call_site())
}
