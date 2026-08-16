//! Adapt between the types the napi wrappers declare and the types the
//! user's own code spells.
//!
//! Only 64-bit integers differ between the two. They cross as a JavaScript
//! `number` -- the policy every mainstream SDK ships, so records stay plain
//! JSON -- declared `f64` in the glue. Every position mentioning one (the
//! value itself, a `Vec`/`Option`/map of them, or a record holding one)
//! needs an explicit adaptation, and inbound the adaptation can reject:
//! a `number` that is fractional, non-finite, or outside the double-exact
//! +/-(2^53 - 1) range is refused, never truncated.
//!
//! Both directions answer `None` for the types that cross unchanged, which
//! keeps the generated glue byte-identical everywhere 64-bit integers do
//! not reach.

use proc_macro2::{Ident, TokenStream};
use quote::{format_ident, quote};
use unibind_core::ir;
use unibind_core::render::{RenderError, name_ident};

use crate::ty::{Level, TyCtx};

/// The integer widths a double cannot hold in full, in the order their
/// narrowing helpers render.
const WIDE_INT_KINDS: [ir::IntKind; 4] = [
    ir::IntKind::I64,
    ir::IntKind::U64,
    ir::IntKind::Isize,
    ir::IntKind::Usize,
];

/// Whether `kind` is wider than an IEEE double holds exactly. Such a width
/// still crosses as a JavaScript `number` -- the Stripe/OpenAI policy, so
/// records stay plain JSON -- but through a checked `f64` adaptation: an
/// inbound value that is fractional or outside the safe-integer range
/// (+/-2^53 - 1) is refused, never truncated. Outbound values convert with
/// `as f64`; the platform's own wide values (epoch milliseconds, byte
/// counts, microcredits) sit far inside the exact range.
pub const fn is_wide_int(kind: ir::IntKind) -> bool {
    matches!(
        kind,
        ir::IntKind::I64 | ir::IntKind::U64 | ir::IntKind::Isize | ir::IntKind::Usize
    )
}

/// The generated mirror struct for record `name`: the `#[napi(object)]`
/// twin the glue owns, so a 64-bit field can be declared `f64` (with its
/// checked conversion) without the user's struct ever mentioning napi.
pub fn mirror_ident(name: &str) -> Ident {
    format_ident!("__UnibindRecord{}", name)
}

/// Whether a value of `ty` at `level` is spelled differently on the two
/// sides of the boundary, given the records already known to cross through
/// a mirror.
///
/// Bytes are the position-sensitive case: a record field of bytes is
/// declared `Buffer`, which the user's own struct never spells, so such a
/// record has to cross through a mirror. Bytes inside a container are
/// declared as the user's own `Vec<u8>` and so adapt nothing.
pub fn adapts(ty: &ir::Type, enums: &[ir::Enum], mirrored: &[String], level: Level) -> bool {
    match ty {
        ir::Type::Int(kind) => is_wide_int(*kind),
        ir::Type::Bytes { .. } => level.bytes_as_buffer(),
        ir::Type::Option(inner) => adapts(inner, enums, mirrored, level),
        ir::Type::Vec(inner) | ir::Type::Stream(inner) => {
            adapts(inner, enums, mirrored, Level::Element)
        }
        ir::Type::Map { value, .. } => adapts(value, enums, mirrored, Level::Element),
        // A unit enum is a Rust enum on one side and a string on the other,
        // so a record holding one is spelled differently on the two sides and
        // has to cross through a mirror, exactly as a 64-bit field does.
        ir::Type::Named(name) => {
            mirrored.iter().any(|mirrored| mirrored == name)
                || enums.iter().any(|declared| declared.name == *name)
        }
        ir::Type::Bool | ir::Type::Float(_) | ir::Type::String { .. } | ir::Type::Path { .. } => {
            false
        }
    }
}

/// A record field's bytes, widened into the `Buffer` its mirror declares.
///
/// Only [`Level::Field`] bytes reach here: the mirror declares them
/// `Buffer` (see [`crate::ty::Level`]), and the conversion is a move of the
/// same allocation, never a copy. `None` for every other field type, which
/// [`outward`] then answers for.
pub fn bytes_field_outward(ty: &ir::Type, expr: &TokenStream) -> Option<TokenStream> {
    Some(match ty {
        ir::Type::Bytes { .. } => quote!(::napi::bindgen_prelude::Buffer::from(#expr)),
        ir::Type::Option(inner) if matches!(**inner, ir::Type::Bytes { .. }) => {
            quote!(#expr.map(::napi::bindgen_prelude::Buffer::from))
        }
        _ => return None,
    })
}

/// The [`bytes_field_outward`] counterpart: the `Buffer` a mirror field
/// received, back as the `Vec<u8>` the user's struct declares.
///
/// Unlike [`inward`], this one cannot refuse -- every `Buffer` is a valid
/// byte string -- so it yields the value itself rather than a
/// `::napi::Result`, and the mirror places no `?` after it.
pub fn bytes_field_inward(ty: &ir::Type, expr: &TokenStream) -> Option<TokenStream> {
    Some(match ty {
        ir::Type::Bytes { .. } => quote!(::std::vec::Vec::from(#expr)),
        ir::Type::Option(inner) if matches!(**inner, ir::Type::Bytes { .. }) => {
            quote!(#expr.map(::std::vec::Vec::from))
        }
        _ => return None,
    })
}

/// Adapt a value the wrapper received into what the user's code takes.
///
/// The expression evaluates to a `::napi::Result`, so the caller places the
/// `?` itself (a record's field conversion cannot borrow the enclosing
/// function's control flow). `None` means the value crosses unchanged.
pub fn inward(ty: &ir::Type, ctx: &TyCtx<'_>, expr: &TokenStream) -> Option<TokenStream> {
    Some(match ty {
        ir::Type::Int(kind) if is_wide_int(*kind) => {
            let narrow = narrow_ident(*kind);
            quote!(#narrow(#expr))
        }
        ir::Type::Named(name) => {
            if ctx.unit_enum(name).is_some() {
                let parse = enum_from_str_ident(name);
                quote!(#parse(#expr))
            } else {
                let mirror = ctx.mirror(name)?;
                quote!(#mirror::__unibind_into(#expr))
            }
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
        ir::Type::Int(kind) if is_wide_int(*kind) => {
            // Outbound is a plain cast: the platform's own wide values sit
            // far inside the double-exact range, and a cast keeps this
            // direction infallible the way the doc above promises. A value
            // past 2^53 would round to the nearest representable double,
            // exactly as it would in any JSON API.
            let _ = kind;
            quote!(#expr as f64)
        }
        ir::Type::Named(name) => {
            if ctx.unit_enum(name).is_some() {
                let render = enum_to_str_ident(name);
                quote!(#render(#expr))
            } else {
                let mirror = ctx.mirror(name)?;
                quote!(#mirror::__unibind_from(#expr))
            }
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
pub fn helpers(interface: &ir::Interface, ctx: &TyCtx<'_>) -> Result<TokenStream, RenderError> {
    let enums = interface
        .enums
        .iter()
        .map(|declared| enum_codec(declared, ctx))
        .collect::<Result<Vec<_>, _>>()?;
    let kinds = inbound_kinds(interface, ctx.mirrored);
    if kinds.is_empty() {
        return Ok(quote!(#(#enums)*));
    }
    let narrowers = kinds.iter().map(|kind| narrow_fn(*kind));
    // The `allow(dead_code)` here and on each narrower covers one shape: a
    // width reachable only through a record the interface declares but never
    // mentions in a signature, whose conversions are then legitimately
    // uncalled (see `crate::mirror`). That is the user's business, not dead
    // glue to report at them.
    Ok(quote! {
        /// A `number` JavaScript sent that the declared Rust width cannot
        /// hold exactly: fractional, non-finite, negative where unsigned,
        /// or outside the double-exact +/-(2^53 - 1) range. Deliberately not
        /// a `__unibind__:` reason: this is a caller mistake, not a boundary
        /// failure the user's error enum declared, so it surfaces as a plain
        /// napi error rather than one of the generated classes.
        #[allow(dead_code)]
        fn __unibind_int_out_of_range(width: &str, value: f64) -> ::napi::Error {
            ::napi::Error::new(
                ::napi::Status::InvalidArg,
                ::std::format!("{} is not a safe integer for a Rust `{}`", value, width),
            )
        }

        #(#narrowers)*
        #(#enums)*
    })
}

/// The identifier of the generated Rust-to-wire renderer for enum `name`.
fn enum_to_str_ident(name: &str) -> Ident {
    format_ident!("__unibind_enum_to_str_{}", name)
}

/// The identifier of the generated wire-to-Rust parser for enum `name`.
fn enum_from_str_ident(name: &str) -> Ident {
    format_ident!("__unibind_enum_from_str_{}", name)
}

/// Both halves of one unit enum's mapping: the Rust variant to its wire
/// string, and back.
///
/// Outbound is total (every variant has a spelling, and lowering refuses two
/// that collide). Inbound is not: JavaScript is free to hand over any string,
/// so an unrecognized one is refused by name, listing the set it should have
/// come from. Deliberately a plain napi error rather than one of the
/// generated exception classes -- passing a word outside a closed set is a
/// caller mistake, not a failure the user's error enum declared.
fn enum_codec(declared: &ir::Enum, ctx: &TyCtx<'_>) -> Result<TokenStream, RenderError> {
    let user = ctx.user;
    let name = name_ident(&declared.name)?;
    let to_str = enum_to_str_ident(&declared.name);
    let from_str = enum_from_str_ident(&declared.name);
    let variants = declared
        .variants
        .iter()
        .map(|variant| name_ident(&variant.name))
        .collect::<Result<Vec<_>, _>>()?;
    let wires: Vec<&str> = declared
        .variants
        .iter()
        .map(|variant| variant.wire.as_str())
        .collect();
    let accepted = wires.join(", ");
    let rejection = format!("is not a {}; expected one of {accepted}", declared.name);
    Ok(quote! {
        #[allow(dead_code)]
        fn #to_str(value: #user::#name) -> ::std::string::String {
            match value {
                #(#user::#name::#variants => #wires,)*
            }
            .to_owned()
        }

        #[allow(dead_code)]
        fn #from_str(value: ::std::string::String) -> ::napi::Result<#user::#name> {
            match value.as_str() {
                #(#wires => ::std::result::Result::Ok(#user::#name::#variants),)*
                other => ::std::result::Result::Err(::napi::Error::new(
                    ::napi::Status::InvalidArg,
                    ::std::format!("`{}` {}", other, #rejection),
                )),
            }
        }
    })
}

/// The widths that cross *into* Rust: every argument, plus every field of a
/// mirrored record (records cross in both directions).
fn inbound_kinds(interface: &ir::Interface, mirrored: &[String]) -> Vec<ir::IntKind> {
    let mut found = Vec::new();
    for function in &interface.functions {
        collect_args(function, &mut found);
    }
    for object in &interface.objects {
        if let Some(constructor) = &object.constructor {
            collect_args(constructor, &mut found);
        }
        for method in &object.methods {
            collect_args(method, &mut found);
        }
    }
    for record in &interface.records {
        if !mirrored.contains(&record.name) {
            continue;
        }
        for field in &record.fields {
            collect(&field.ty, &mut found);
        }
    }
    WIDE_INT_KINDS
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
        ir::Type::Int(kind) if is_wide_int(*kind) => found.push(*kind),
        ir::Type::Option(inner) | ir::Type::Vec(inner) | ir::Type::Stream(inner) => {
            collect(inner, found);
        }
        ir::Type::Map { value, .. } => collect(value, found),
        _ => {}
    }
}

fn narrow_ident(kind: ir::IntKind) -> Ident {
    format_ident!("__unibind_number_to_{}", kind.rust_name())
}

/// One narrowing helper: a JavaScript `number` into the declared Rust
/// width. Fractional, non-finite, and unsafe-range values are refused,
/// never truncated -- the check is `Number.isSafeInteger` spelled in Rust.
/// Inside the safe range every integer is exact in a double, so the final
/// cast loses nothing; the pointer-sized widths then re-narrow, which is a
/// no-op on 64-bit targets and a real bound on 32-bit ones.
fn narrow_fn(kind: ir::IntKind) -> TokenStream {
    let name = narrow_ident(kind);
    let rust_name = kind.rust_name();
    let target = Ident::new(rust_name, proc_macro2::Span::call_site());
    let doc = format!(
        " Narrow a JavaScript `number` to `{rust_name}`, refusing a value \
         that is not a safe integer in the width instead of truncating it."
    );
    let signed_ok = matches!(kind, ir::IntKind::I64 | ir::IntKind::Isize);
    let sign_check = if signed_ok {
        TokenStream::new()
    } else {
        quote! {
            if value < 0.0 {
                return ::std::result::Result::Err(__unibind_int_out_of_range(#rust_name, value));
            }
        }
    };
    let narrowed = match kind {
        ir::IntKind::I64 => quote!(::std::result::Result::Ok(value as i64)),
        ir::IntKind::U64 => quote!(::std::result::Result::Ok(value as u64)),
        ir::IntKind::Usize => quote! {
            <usize as ::std::convert::TryFrom<u64>>::try_from(value as u64)
                .map_err(|_| __unibind_int_out_of_range(#rust_name, value))
        },
        _ => quote! {
            <isize as ::std::convert::TryFrom<i64>>::try_from(value as i64)
                .map_err(|_| __unibind_int_out_of_range(#rust_name, value))
        },
    };
    quote! {
        #[doc = #doc]
        #[allow(dead_code)]
        fn #name(value: f64) -> ::napi::Result<#target> {
            const MAX_SAFE_INTEGER: f64 = 9_007_199_254_740_991.0;
            if !value.is_finite()
                || value.fract() != 0.0
                || value.abs() > MAX_SAFE_INTEGER
            {
                return ::std::result::Result::Err(__unibind_int_out_of_range(#rust_name, value));
            }
            #sign_check
            #narrowed
        }
    }
}
