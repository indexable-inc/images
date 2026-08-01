//! Adapt between the types the napi wrappers declare and the types the
//! user's own code spells.
//!
//! Only 64-bit integers differ between the two. JavaScript's `number` is an
//! IEEE double, so it holds integers exactly only up to 2^53; anything
//! wider crosses as a `bigint`, which napi carries as
//! `napi::bindgen_prelude::BigInt`. Every position mentioning one -- the
//! value itself, a `Vec`/`Option`/map of them, or a record holding one --
//! needs an explicit adaptation, and inbound the adaptation can reject
//! (a `bigint` outside the declared Rust width is refused, never
//! truncated).
//!
//! Both directions answer `None` for the types that cross unchanged, which
//! keeps the generated glue byte-identical everywhere 64-bit integers do
//! not reach.

use proc_macro2::{Ident, TokenStream};
use quote::{format_ident, quote};
use unibind_core::ir;

use crate::ty::TyCtx;

/// The integer widths JavaScript can only carry as `bigint`, in the order
/// their narrowing helpers render.
const BIGINT_KINDS: [ir::IntKind; 4] = [
    ir::IntKind::I64,
    ir::IntKind::U64,
    ir::IntKind::Isize,
    ir::IntKind::Usize,
];

/// Whether `kind` is wider than an IEEE double holds exactly, and so
/// crosses as `bigint` rather than `number`.
pub const fn is_bigint(kind: ir::IntKind) -> bool {
    matches!(
        kind,
        ir::IntKind::I64 | ir::IntKind::U64 | ir::IntKind::Isize | ir::IntKind::Usize
    )
}

/// The generated mirror struct for record `name`: the `#[napi(object)]`
/// twin the glue owns, so a 64-bit field can be declared `BigInt` without
/// the user's struct ever mentioning napi.
pub fn mirror_ident(name: &str) -> Ident {
    format_ident!("__UnibindRecord{}", name)
}

/// Whether a value of `ty` is spelled differently on the two sides of the
/// boundary, given the records already known to cross through a mirror.
pub fn adapts(ty: &ir::Type, mirrored: &[String]) -> bool {
    match ty {
        ir::Type::Int(kind) => is_bigint(*kind),
        ir::Type::Option(inner) | ir::Type::Vec(inner) | ir::Type::Stream(inner) => {
            adapts(inner, mirrored)
        }
        ir::Type::Map { value, .. } => adapts(value, mirrored),
        ir::Type::Named(name) => mirrored.iter().any(|mirrored| mirrored == name),
        ir::Type::Bool
        | ir::Type::Float(_)
        | ir::Type::String { .. }
        | ir::Type::Path { .. }
        | ir::Type::Bytes { .. } => false,
    }
}

/// Adapt a value the wrapper received into what the user's code takes.
///
/// The expression evaluates to a `::napi::Result`, so the caller places the
/// `?` itself (a record's field conversion cannot borrow the enclosing
/// function's control flow). `None` means the value crosses unchanged.
pub fn inward(ty: &ir::Type, ctx: &TyCtx<'_>, expr: &TokenStream) -> Option<TokenStream> {
    Some(match ty {
        ir::Type::Int(kind) if is_bigint(*kind) => {
            let narrow = narrow_ident(*kind);
            quote!(#narrow(#expr))
        }
        ir::Type::Named(name) => {
            let mirror = ctx.mirror(name)?;
            quote!(#mirror::__unibind_into(#expr))
        }
        ir::Type::Option(inner) => {
            let element = inward(inner, ctx, &quote!(__unibind_element))?;
            quote!(#expr.map(|__unibind_element| #element).transpose())
        }
        ir::Type::Vec(inner) => {
            let element = inward(inner, ctx, &quote!(__unibind_element))?;
            quote! {
                #expr
                    .into_iter()
                    .map(|__unibind_element| #element)
                    .collect::<::napi::Result<::std::vec::Vec<_>>>()
            }
        }
        ir::Type::Map { value, .. } => {
            let element = inward(value, ctx, &quote!(__unibind_element))?;
            quote! {
                #expr
                    .into_iter()
                    .map(|(__unibind_key, __unibind_element)| {
                        #element.map(|__unibind_element| (__unibind_key, __unibind_element))
                    })
                    .collect::<::napi::Result<::std::collections::HashMap<_, _>>>()
            }
        }
        _ => return None,
    })
}

/// Adapt a value the user's code produced into what the wrapper declares.
/// Every widening is exact, so this direction never fails.
pub fn outward(ty: &ir::Type, ctx: &TyCtx<'_>, expr: &TokenStream) -> Option<TokenStream> {
    Some(match ty {
        ir::Type::Int(kind) if is_bigint(*kind) => {
            // `BigInt` converts from the fixed widths; the pointer-sized
            // ones widen first, which is exact on every target Node runs on.
            let widened = match kind {
                ir::IntKind::Usize => quote!(#expr as u64),
                ir::IntKind::Isize => quote!(#expr as i64),
                _ => expr.clone(),
            };
            quote!(::napi::bindgen_prelude::BigInt::from(#widened))
        }
        ir::Type::Named(name) => {
            let mirror = ctx.mirror(name)?;
            quote!(#mirror::__unibind_from(#expr))
        }
        ir::Type::Option(inner) => {
            let element = outward(inner, ctx, &quote!(__unibind_element))?;
            quote!(#expr.map(|__unibind_element| #element))
        }
        ir::Type::Vec(inner) => {
            let element = outward(inner, ctx, &quote!(__unibind_element))?;
            quote! {
                #expr
                    .into_iter()
                    .map(|__unibind_element| #element)
                    .collect::<::std::vec::Vec<_>>()
            }
        }
        ir::Type::Map { value, .. } => {
            let element = outward(value, ctx, &quote!(__unibind_element))?;
            quote! {
                #expr
                    .into_iter()
                    .map(|(__unibind_key, __unibind_element)| (__unibind_key, #element))
                    .collect::<::std::collections::HashMap<_, _>>()
            }
        }
        _ => return None,
    })
}

/// The narrowing helpers the glue module needs: one per 64-bit width that
/// actually arrives from JavaScript, plus the shared rejection they raise.
/// Emitting only the reachable ones keeps the module free of dead code.
pub fn helpers(interface: &ir::Interface, mirrored: &[String]) -> TokenStream {
    let kinds = inbound_kinds(interface, mirrored);
    if kinds.is_empty() {
        return TokenStream::new();
    }
    let narrowers = kinds.iter().map(|kind| narrow_fn(*kind));
    quote! {
        /// A `bigint` JavaScript sent that the declared Rust width cannot
        /// hold. Deliberately not a `__unibind__:` reason: this is a caller
        /// mistake, not a boundary failure the user's error enum declared,
        /// so it surfaces as a plain napi error rather than one of the
        /// generated classes.
        fn __unibind_bigint_out_of_range(width: &str) -> ::napi::Error {
            ::napi::Error::new(
                ::napi::Status::InvalidArg,
                ::std::format!("bigint does not fit in a Rust `{}`", width),
            )
        }

        #(#narrowers)*
    }
}

/// The widths that cross *into* Rust: every argument, plus every field of a
/// mirrored record (records cross in both directions).
fn inbound_kinds(interface: &ir::Interface, mirrored: &[String]) -> Vec<ir::IntKind> {
    let mut found = Vec::new();
    for function in &interface.functions {
        collect_args(function, &mut found);
    }
    for object in &interface.objects {
        for constructor in &object.constructor {
            collect_args(constructor, &mut found);
        }
        for method in &object.methods {
            collect_args(method, &mut found);
        }
    }
    for record in &interface.records {
        if !mirrored.iter().any(|name| *name == record.name) {
            continue;
        }
        for field in &record.fields {
            collect(&field.ty, &mut found);
        }
    }
    BIGINT_KINDS
        .into_iter()
        .filter(|kind| {
            found
                .iter()
                .any(|seen| seen.rust_name() == kind.rust_name())
        })
        .collect()
}

fn collect_args(function: &ir::Function, found: &mut Vec<ir::IntKind>) {
    for arg in &function.args {
        collect(&arg.ty, found);
    }
}

fn collect(ty: &ir::Type, found: &mut Vec<ir::IntKind>) {
    match ty {
        ir::Type::Int(kind) if is_bigint(*kind) => found.push(*kind),
        ir::Type::Option(inner) | ir::Type::Vec(inner) | ir::Type::Stream(inner) => {
            collect(inner, found);
        }
        ir::Type::Map { value, .. } => collect(value, found),
        _ => {}
    }
}

fn narrow_ident(kind: ir::IntKind) -> Ident {
    format_ident!("__unibind_bigint_to_{}", kind.rust_name())
}

/// One narrowing helper. `BigInt::get_i64`/`get_u64` report whether the
/// value survived the first word intact; the pointer-sized widths then
/// re-narrow, which is a no-op on 64-bit targets and a real bound on
/// 32-bit ones.
fn narrow_fn(kind: ir::IntKind) -> TokenStream {
    let name = narrow_ident(kind);
    let rust_name = kind.rust_name();
    let target = Ident::new(rust_name, proc_macro2::Span::call_site());
    let doc = format!(
        " Narrow a JavaScript `bigint` to `{rust_name}`, refusing a value \
         outside the width instead of truncating it."
    );
    // `get_u64` reports "exact" only for a single-word, non-negative value,
    // so an unsigned width needs no separate sign check.
    let read = match kind {
        ir::IntKind::U64 | ir::IntKind::Usize => quote! {
            let (_, __unibind_value, __unibind_exact) = value.get_u64();
        },
        _ => quote! {
            let (__unibind_value, __unibind_exact) = value.get_i64();
        },
    };
    let narrowed = match kind {
        ir::IntKind::I64 | ir::IntKind::U64 => quote!(::std::result::Result::Ok(__unibind_value)),
        ir::IntKind::Usize => quote! {
            <usize as ::std::convert::TryFrom<u64>>::try_from(__unibind_value)
                .map_err(|_| __unibind_bigint_out_of_range(#rust_name))
        },
        _ => quote! {
            <isize as ::std::convert::TryFrom<i64>>::try_from(__unibind_value)
                .map_err(|_| __unibind_bigint_out_of_range(#rust_name))
        },
    };
    quote! {
        #[doc = #doc]
        fn #name(value: ::napi::bindgen_prelude::BigInt) -> ::napi::Result<#target> {
            #read
            if !__unibind_exact {
                return ::std::result::Result::Err(__unibind_bigint_out_of_range(#rust_name));
            }
            #narrowed
        }
    }
}
