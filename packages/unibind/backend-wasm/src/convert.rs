//! Adapt between the types the `wasm-bindgen` wrappers declare and the types
//! the user's own code spells.
//!
//! Three things differ between the two sides, and every position mentioning
//! one needs an explicit adaptation:
//!
//! - **64-bit integers.** They cross as a JavaScript `number` -- the policy
//!   every mainstream SDK ships, so records stay plain JSON -- declared `f64`
//!   in the glue. Inbound the adaptation can reject: a `number` that is
//!   fractional, non-finite, or outside the double-exact +/-(2^53 - 1) range is
//!   refused, never truncated.
//! - **Unit enums.** A Rust enum on one side, its wire string on the other.
//!   Inbound refuses a string outside the declared set, listing it.
//! - **Records.** They cross through the generated serde twin
//!   ([`crate::twin`]), which is also where a nested record, enum, or 64-bit
//!   field gets its own adaptation.
//!
//! Everything a conversion refuses is a plain reason `String`, not a
//! `JsValue`: a twin's conversions run inside serde's own machinery, where
//! there is no boundary yet, and the wrapper that owns the boundary wraps the
//! reason into a `js_sys::Error` at the one place it becomes one. Both
//! directions answer `None` for the types that cross unchanged, which keeps
//! the generated glue byte-identical everywhere an adaptation does not reach.

use proc_macro2::{Ident, TokenStream};
use quote::{format_ident, quote};
use unibind_core::ir;
use unibind_core::render::{RenderError, name_ident};

use crate::ty::{self, Level, TyCtx};

/// The integer widths a double cannot hold in full, in the order their
/// narrowing helpers render.
const WIDE_INT_KINDS: [ir::IntKind; 4] = [
    ir::IntKind::I64,
    ir::IntKind::U64,
    ir::IntKind::Isize,
    ir::IntKind::Usize,
];

/// Whether `kind` is wider than an IEEE double holds exactly. Such a width
/// still crosses as a JavaScript `number` -- never a `bigint`, so node and the
/// browser share one `.d.ts` vocabulary -- but through a checked `f64`
/// adaptation: an inbound value that is fractional or outside the safe-integer
/// range (+/-2^53 - 1) is refused, never truncated. Outbound values convert
/// with `as f64`; the platform's own wide values (epoch milliseconds, byte
/// counts, microcredits) sit far inside the exact range.
pub const fn is_wide_int(kind: ir::IntKind) -> bool {
    matches!(
        kind,
        ir::IntKind::I64 | ir::IntKind::U64 | ir::IntKind::Isize | ir::IntKind::Usize
    )
}

/// The generated serde twin for record `name` (see [`crate::twin`]).
pub fn twin_ident(name: &str) -> Ident {
    format_ident!("__UnibindWasmRecord{}", name)
}

/// A path on its way out, as the JavaScript string a signature declares.
///
/// Fallible on purpose, and asked before [`outward`] for the same reason the
/// ts backend asks about bytes first: a path that is not valid UTF-8 has no
/// JavaScript string to become, and serde refuses such a path in a twin field
/// with the same verdict, so the two positions agree. `None` for every other
/// type, which [`outward`] then answers for.
pub fn path_outward(ty: &ir::Type, expr: &TokenStream) -> Option<TokenStream> {
    Some(match ty {
        ir::Type::Path { .. } => quote!(__unibind_wasm_path_to_string(#expr)),
        ir::Type::Option(inner) if matches!(**inner, ir::Type::Path { .. }) => {
            quote!(#expr.map(__unibind_wasm_path_to_string).transpose())
        }
        _ => return None,
    })
}

/// Adapt a whole argument the wrapper received into what the user's code
/// takes.
///
/// The expression evaluates to a `Result<_, String>`, so the caller places the
/// `?` (and the wrap into a `JsValue`) itself. `None` means the value crosses
/// unchanged apart from the reborrowing [`ty::pass`] does.
///
/// # Errors
///
/// Fails when the argument's serde spelling cannot be rendered, which is the
/// surface lowering already refuses (a stream or an object anywhere but a bare
/// return).
pub fn inward(
    ty: &ir::Type,
    ctx: &TyCtx<'_>,
    expr: &TokenStream,
) -> Result<Option<TokenStream>, RenderError> {
    if ty::crosses_natively(ty, ctx) {
        return Ok(inward_serde(ty, ctx, expr));
    }
    // A structured argument arrives as one `JsValue`: decode it into the serde
    // spelling first, then convert that into the user's own types.
    let serde_ty = ty::decl(ty, ctx, Level::Inner)?;
    let decoded = quote!(__unibind_wasm_from_js::<#serde_ty>(#expr));
    Ok(Some(
        inward_serde(ty, ctx, &quote!(__unibind_value)).map_or_else(
            || decoded.clone(),
            |converted| quote!(#decoded.and_then(|__unibind_value| #converted)),
        ),
    ))
}

/// Adapt a value in its serde spelling into the user's own type; the
/// conversion inside a twin field, a list element, or a map value.
///
/// Yields a `Result<_, String>`; `None` for the types serde already spells the
/// way the user's code does.
pub fn inward_serde(ty: &ir::Type, ctx: &TyCtx<'_>, expr: &TokenStream) -> Option<TokenStream> {
    Some(match ty {
        ir::Type::Int(kind) if is_wide_int(*kind) => {
            let narrow = narrow_ident(*kind);
            quote!(#narrow(#expr))
        }
        ir::Type::Named(name) => {
            if ctx.unit_enum(name).is_some() {
                let parse = enum_from_str_ident(name);
                quote!(#parse(#expr))
            } else if ctx.object(name).is_some() {
                // An object is a live handle, never data; lowering confines it
                // to a bare return, so nothing reaches here.
                return None;
            } else {
                let twin = twin_ident(name);
                quote!(#twin::__unibind_into(#expr))
            }
        }
        ir::Type::Option(inner) => {
            let element = inward_serde(inner, ctx, &quote!(__unibind_element))?;
            quote!(#expr.map(|__unibind_element| #element).transpose())
        }
        ir::Type::Vec(inner) => {
            let element = inward_serde(inner, ctx, &quote!(__unibind_element))?;
            quote! {
                #expr
                    .into_iter()
                    .map(|__unibind_element| #element)
                    .collect::<
                        ::std::result::Result<
                            ::std::vec::Vec<_>,
                            ::std::string::String,
                        >
                    >()
            }
        }
        ir::Type::Map { value, .. } => {
            let element = inward_serde(value, ctx, &quote!(__unibind_element))?;
            quote! {
                #expr
                    .into_iter()
                    .map(|(__unibind_key, __unibind_element)| {
                        #element.map(|__unibind_element| (__unibind_key, __unibind_element))
                    })
                    .collect::<
                        ::std::result::Result<
                            ::std::collections::HashMap<_, _>,
                            ::std::string::String,
                        >
                    >()
            }
        }
        _ => return None,
    })
}

/// Adapt a value the user's code produced into its serde spelling (which is
/// also its native spelling wherever the two agree). Every widening is exact,
/// so this direction never fails; paths are [`path_outward`]'s business.
pub fn outward(ty: &ir::Type, ctx: &TyCtx<'_>, expr: &TokenStream) -> Option<TokenStream> {
    Some(match ty {
        ir::Type::Int(kind) if is_wide_int(*kind) => {
            // A plain cast: the platform's own wide values sit far inside the
            // double-exact range, and the cast keeps this direction infallible
            // the way the doc above promises. A value past 2^53 rounds to the
            // nearest representable double, exactly as it would in any JSON API.
            let _ = kind;
            quote!(#expr as f64)
        }
        ir::Type::Named(name) => {
            if ctx.unit_enum(name).is_some() {
                let render = enum_to_str_ident(name);
                quote!(#render(#expr))
            } else if ctx.object(name).is_some() {
                return None;
            } else {
                let twin = twin_ident(name);
                quote!(#twin::__unibind_from(#expr))
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

/// Everything the glue module needs before its wrappers: the boundary
/// vocabulary, one narrowing helper per 64-bit width the interface mentions,
/// and both halves of every unit enum's mapping.
///
/// # Errors
///
/// Fails for an enum name or variant that cannot become an identifier.
pub fn helpers(interface: &ir::Interface, ctx: &TyCtx<'_>) -> Result<TokenStream, RenderError> {
    let enums = interface
        .enums
        .iter()
        .map(|declared| enum_codec(declared, ctx))
        .collect::<Result<Vec<_>, _>>()?;
    let boundary = boundary();
    let kinds = int_kinds(interface);
    let narrowers = (!kinds.is_empty()).then(|| {
        let narrowers = kinds.iter().map(|kind| narrow_fn(*kind));
        quote! {
            /// A `number` JavaScript sent that the declared Rust width cannot
            /// hold exactly: fractional, non-finite, negative where unsigned,
            /// or outside the double-exact +/-(2^53 - 1) range. Deliberately
            /// not a `__unibind__:` reason: this is a caller mistake, not a
            /// boundary failure the user's error enum declared.
            #[allow(dead_code)]
            fn __unibind_wasm_int_out_of_range(
                width: &str,
                value: f64,
            ) -> ::std::string::String {
                ::std::format!("{} is not a safe integer for a Rust `{}`", value, width)
            }

            #(#narrowers)*
        }
    });
    Ok(quote! {
        #boundary
        #narrowers
        #(#enums)*
    })
}

/// The vocabulary every wrapper shares: the one place a reason string becomes
/// a JavaScript error, and the two serde moves.
///
/// Emitted unconditionally rather than per reachable position. A walk that has
/// to enumerate every position a helper is reachable from is a walk that can
/// be wrong, and being wrong here means glue that does not compile; these four
/// are small, and `allow(dead_code)` covers an interface that never moves a
/// structured value.
fn boundary() -> TokenStream {
    quote! {
        /// A conversion's reason string as the `JsValue` a wrapper rejects
        /// with. Every refusal the glue raises passes through here, so the
        /// error channel has exactly one spelling.
        #[allow(dead_code)]
        fn __unibind_wasm_error(reason: ::std::string::String) -> ::wasm_bindgen::JsValue {
            ::wasm_bindgen::JsValue::from(::js_sys::Error::new(&reason))
        }

        /// One structured argument, out of the `JsValue` JavaScript sent.
        #[allow(dead_code)]
        fn __unibind_wasm_from_js<__UnibindValue>(
            value: ::wasm_bindgen::JsValue,
        ) -> ::std::result::Result<__UnibindValue, ::std::string::String>
        where
            __UnibindValue: ::serde::de::DeserializeOwned,
        {
            ::serde_wasm_bindgen::from_value(value)
                .map_err(|error| ::std::string::ToString::to_string(&error))
        }

        /// One structured value, into the `JsValue` JavaScript receives.
        ///
        /// `json_compatible` picks the shape the ts backend's napi records
        /// already have: a map is a plain object rather than an ES `Map`, and
        /// an absent value is `null`. Two backends, one wire vocabulary.
        #[allow(dead_code)]
        fn __unibind_wasm_to_js<__UnibindValue>(
            value: &__UnibindValue,
        ) -> ::std::result::Result<::wasm_bindgen::JsValue, ::std::string::String>
        where
            __UnibindValue: ::serde::Serialize + ?Sized,
        {
            let serializer = ::serde_wasm_bindgen::Serializer::json_compatible();
            ::serde::Serialize::serialize(value, &serializer)
                .map_err(|error| ::std::string::ToString::to_string(&error))
        }

        /// A path as the JavaScript string a signature declares. Refuses a
        /// path that is not valid UTF-8, the same verdict serde reaches for a
        /// path inside a record.
        #[allow(dead_code)]
        fn __unibind_wasm_path_to_string(
            path: ::std::path::PathBuf,
        ) -> ::std::result::Result<::std::string::String, ::std::string::String> {
            path.into_os_string().into_string().map_err(|_| {
                ::std::string::String::from(
                    "a path that is not valid UTF-8 cannot cross to JavaScript",
                )
            })
        }
    }
}

/// The identifier of the generated Rust-to-wire renderer for enum `name`.
fn enum_to_str_ident(name: &str) -> Ident {
    format_ident!("__unibind_wasm_enum_to_str_{}", name)
}

/// The identifier of the generated wire-to-Rust parser for enum `name`.
fn enum_from_str_ident(name: &str) -> Ident {
    format_ident!("__unibind_wasm_enum_from_str_{}", name)
}

/// Both halves of one unit enum's mapping: the Rust variant to its wire
/// string, and back.
///
/// Outbound is total (every variant has a spelling, and lowering refuses two
/// that collide). Inbound is not: JavaScript is free to hand over any string,
/// so an unrecognized one is refused by name, listing the set it should have
/// come from. Deliberately not a `__unibind__:` reason -- passing a word
/// outside a closed set is a caller mistake, not a failure the user's error
/// enum declared.
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
        fn #from_str(
            value: ::std::string::String,
        ) -> ::std::result::Result<#user::#name, ::std::string::String> {
            match value.as_str() {
                #(#wires => ::std::result::Result::Ok(#user::#name::#variants),)*
                other => ::std::result::Result::Err(
                    ::std::format!("`{}` {}", other, #rejection),
                ),
            }
        }
    })
}

/// The 64-bit widths the interface mentions anywhere: arguments, returns,
/// stream items, and record fields.
///
/// Deliberately wider than "the widths that cross into Rust": every record
/// crosses through a twin whose conversions run in both directions, and a
/// walk that has to enumerate every inbound position is a walk that can be
/// wrong -- the ts backend's forgets an object's associated functions, which
/// leaves the glue calling a narrower it never emitted. An unused narrower is
/// only an `allow(dead_code)`, so this walk answers the question it can
/// answer completely.
fn int_kinds(interface: &ir::Interface) -> Vec<ir::IntKind> {
    let mut found = Vec::new();
    each_type(interface, &mut |ty| {
        ty.for_each_leaf(&mut |leaf| {
            if let ir::Type::Int(kind) = leaf
                && is_wide_int(*kind)
            {
                found.push(*kind);
            }
        });
    });
    WIDE_INT_KINDS
        .into_iter()
        .filter(|kind| {
            found
                .iter()
                .any(|seen| seen.rust_name() == kind.rust_name())
        })
        .collect()
}

/// Visit every boundary type the interface declares, in render order.
fn each_type(interface: &ir::Interface, visit: &mut impl FnMut(&ir::Type)) {
    let members = interface.objects.iter().flat_map(|object| {
        object
            .constructor
            .iter()
            .chain(object.associated.iter())
            .chain(object.methods.iter())
    });
    for function in interface.functions.iter().chain(members) {
        for arg in &function.args {
            visit(&arg.ty);
        }
        if let Some(ret) = &function.ret {
            visit(ret);
        }
    }
    for record in &interface.records {
        for field in &record.fields {
            visit(&field.ty);
        }
    }
}

fn narrow_ident(kind: ir::IntKind) -> Ident {
    format_ident!("__unibind_wasm_number_to_{}", kind.rust_name())
}

/// One narrowing helper: a JavaScript `number` into the declared Rust width.
/// Fractional, non-finite, and unsafe-range values are refused, never
/// truncated -- the check is `Number.isSafeInteger` spelled in Rust. Inside
/// the safe range every integer is exact in a double, so the final cast loses
/// nothing; the pointer-sized widths then re-narrow, which is a no-op on
/// 64-bit targets and a real bound on the 32-bit one wasm actually runs.
fn narrow_fn(kind: ir::IntKind) -> TokenStream {
    let name = narrow_ident(kind);
    let rust_name = kind.rust_name();
    let target = Ident::new(rust_name, proc_macro2::Span::call_site());
    let doc = format!(
        " Narrow a JavaScript `number` to `{rust_name}`, refusing a value that \
         is not a safe integer in the width instead of truncating it."
    );
    let signed_ok = matches!(kind, ir::IntKind::I64 | ir::IntKind::Isize);
    let sign_check = if signed_ok {
        TokenStream::new()
    } else {
        quote! {
            if value < 0.0 {
                return ::std::result::Result::Err(
                    __unibind_wasm_int_out_of_range(#rust_name, value),
                );
            }
        }
    };
    let narrowed = match kind {
        ir::IntKind::I64 => quote!(::std::result::Result::Ok(value as i64)),
        ir::IntKind::U64 => quote!(::std::result::Result::Ok(value as u64)),
        ir::IntKind::Usize => quote! {
            <usize as ::std::convert::TryFrom<u64>>::try_from(value as u64)
                .map_err(|_| __unibind_wasm_int_out_of_range(#rust_name, value))
        },
        _ => quote! {
            <isize as ::std::convert::TryFrom<i64>>::try_from(value as i64)
                .map_err(|_| __unibind_wasm_int_out_of_range(#rust_name, value))
        },
    };
    quote! {
        #[doc = #doc]
        #[allow(dead_code)]
        fn #name(value: f64) -> ::std::result::Result<#target, ::std::string::String> {
            const MAX_SAFE_INTEGER: f64 = 9_007_199_254_740_991.0;
            if !value.is_finite() || value.fract() != 0.0 || value.abs() > MAX_SAFE_INTEGER {
                return ::std::result::Result::Err(
                    __unibind_wasm_int_out_of_range(#rust_name, value),
                );
            }
            #sign_check
            #narrowed
        }
    }
}
