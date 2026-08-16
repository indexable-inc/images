//! Render IR types into the Rust tokens of the generated `wasm-bindgen` glue.
//!
//! Two spellings exist for every boundary type, and [`Level`] is which one a
//! position gets: the *native* spelling `wasm-bindgen` carries in a signature,
//! and the *serde* spelling that rides inside a `JsValue`. A value crosses
//! natively when `wasm-bindgen` has a faithful ABI for it ([`crosses_natively`]);
//! everything with structure crosses through `serde_wasm_bindgen` instead.

use proc_macro2::{Ident, Span, TokenStream};
use quote::quote;
use unibind_core::ir;
use unibind_core::render::{RenderError, pascal_case};

use crate::convert;

/// Interface-wide context the type mapping needs: the alias the glue binds to
/// the exported module (named types resolve through `<user>::`), the declared
/// objects (which map to generated handle classes, not user structs), and the
/// declared unit enums (which cross as their wire string).
pub struct TyCtx<'a> {
    /// The glue-level alias for the user's module, never `super::<module>`
    /// directly: see the binding in [`crate::module`] for why.
    pub user: &'a Ident,
    pub objects: &'a [ir::Object],
    pub enums: &'a [ir::Enum],
}

impl TyCtx<'_> {
    pub fn object(&self, name: &str) -> Option<&ir::Object> {
        self.objects.iter().find(|object| object.name == name)
    }

    /// The unit enum a [`ir::Type::Named`] refers to, when it names one. A
    /// declared enum crosses as its wire string, so this is what tells the
    /// declaration and both conversions apart from a record.
    pub fn unit_enum(&self, name: &str) -> Option<&ir::Enum> {
        self.enums.iter().find(|declared| declared.name == name)
    }
}

/// Which spelling a position takes.
///
/// The dividing line is whether `wasm-bindgen` itself carries the value. A
/// whole argument, return value, or stream item is a `wasm-bindgen` signature
/// position, so a byte string there is a `Vec<u8>` -- a `Uint8Array` on the
/// JavaScript side, with no copy through JSON. Everything reachable only
/// *inside* a `JsValue` is serde's: a twin record's field, a `Vec` element, a
/// map value, where the same `Vec<u8>` is an array of numbers because that is
/// what `serde` makes of it.
///
/// The ts backend needs a third case (a record field, which napi declares as
/// its own `Buffer`); serde has no such carve-out, so two cases cover it here.
/// The two places the split is actually spent are [`decl`], where a path is a
/// `String` at the top and the user's own `PathBuf` inside serde, and
/// [`js_value`], which exists for [`Level::Top`] alone and is where a byte
/// string becomes a `Uint8Array` view instead of an array of numbers.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Level {
    /// A whole argument, return value, or stream item.
    Top,
    /// Inside a serde payload: a twin's field, a `Vec` element, a map value.
    Inner,
}

/// One return value's crossing: what the wrapper declares, the expression
/// producing it from `value`, and whether that expression can refuse.
///
/// A refusal is a `Result<_, String>`; the caller decides where the reason
/// becomes a `JsValue` (a sync wrapper maps it, a `Promise`-returning one
/// rejects with it). Serde can refuse for any value it moves, and a path that
/// is not valid UTF-8 has no JavaScript string to become, so `fallible` is not
/// a property of the *user's* signature the way `throws` is.
pub struct Returned {
    /// The success type the wrapper declares.
    pub decl: TokenStream,
    /// The expression producing it, written against `value`.
    pub value: TokenStream,
    /// Whether [`Self::value`] is a `Result<_, String>`.
    pub fallible: bool,
}

/// Whether `wasm-bindgen` carries a whole value of this type in a signature.
///
/// True for the primitives, strings, paths (as strings), byte strings, an
/// `Option` of any of those, a declared object (its generated class), and a
/// unit enum (its wire string). False for anything with structure, which
/// crosses as one `JsValue` through serde instead.
pub fn crosses_natively(ty: &ir::Type, ctx: &TyCtx<'_>) -> bool {
    match ty {
        ir::Type::Bool
        | ir::Type::Int(_)
        | ir::Type::Float(_)
        | ir::Type::String { .. }
        | ir::Type::Path { .. }
        | ir::Type::Bytes { .. } => true,
        ir::Type::Option(inner) => crosses_natively(inner, ctx),
        ir::Type::Named(name) => {
            ctx.object(name).is_some() || ctx.unit_enum(name).is_some()
        }
        ir::Type::Vec(_) | ir::Type::Map { .. } | ir::Type::Stream(_) => false,
    }
}

/// Reject the type surface the wasm boundary cannot represent faithfully.
/// Walks nested types; run it before spelling any tokens so failures name the
/// follow-up instead of miscompiling.
pub fn check(ty: &ir::Type, what: &str) -> Result<(), RenderError> {
    match ty {
        ir::Type::Map { key, value } => {
            if !matches!(**key, ir::Type::String { .. }) {
                return Err(RenderError::new(format!(
                    "{what} is a map with non-string keys; a JavaScript object \
                     key is a string and `serde_wasm_bindgen` carries the map \
                     as one, so integer-keyed maps are not part of the wasm \
                     backend yet (issue #1993)"
                )));
            }
            check(value, what)
        }
        ir::Type::Option(inner) | ir::Type::Vec(inner) | ir::Type::Stream(inner) => {
            check(inner, what)
        }
        ir::Type::Bool
        | ir::Type::Int(_)
        | ir::Type::Float(_)
        | ir::Type::String { .. }
        | ir::Type::Path { .. }
        | ir::Type::Bytes { .. }
        | ir::Type::Named(_) => Ok(()),
    }
}

/// The Rust type a wrapper declares for a value crossing at `level`.
///
/// At [`Level::Top`] anything without a native ABI collapses to `JsValue`
/// before the match runs, so the container arms below only ever spell serde's
/// interior. Streams never reach here: they cross as the generated per-export
/// class, which [`crate::function`] names before asking for a declaration.
pub fn decl(ty: &ir::Type, ctx: &TyCtx<'_>, level: Level) -> Result<TokenStream, RenderError> {
    if level == Level::Top && !crosses_natively(ty, ctx) {
        return Ok(quote!(::wasm_bindgen::JsValue));
    }
    Ok(match ty {
        ir::Type::Bool => quote!(bool),
        ir::Type::Int(kind) => int_tokens(*kind),
        ir::Type::Float(ir::FloatKind::F32) => quote!(f32),
        ir::Type::Float(ir::FloatKind::F64) => quote!(f64),
        ir::Type::String { .. } => quote!(::std::string::String),
        // A path is a JavaScript string in a signature; serde spells its own
        // (`PathBuf` serializes as a string), so the twin keeps the user's type.
        ir::Type::Path { .. } => match level {
            Level::Top => quote!(::std::string::String),
            Level::Inner => quote!(::std::path::PathBuf),
        },
        ir::Type::Bytes { .. } => quote!(::std::vec::Vec<u8>),
        ir::Type::Option(inner) => {
            let inner = decl(inner, ctx, level)?;
            quote!(::std::option::Option<#inner>)
        }
        ir::Type::Vec(inner) => {
            let inner = decl(inner, ctx, Level::Inner)?;
            quote!(::std::vec::Vec<#inner>)
        }
        ir::Type::Map { value, .. } => {
            let value = decl(value, ctx, Level::Inner)?;
            quote!(::std::collections::HashMap<::std::string::String, #value>)
        }
        ir::Type::Named(name) => named_decl(name, ctx, level),
        ir::Type::Stream(_) => {
            return Err(RenderError::new(
                "streams cross only as a whole return type; the wrapper never \
                 spells UniStream in a wasm-bindgen signature"
                    .to_owned(),
            ));
        }
    })
}

fn named_decl(name: &str, ctx: &TyCtx<'_>, level: Level) -> TokenStream {
    if let Some(object) = ctx.object(name) {
        let handle = object_handle_ident(object);
        return quote!(#handle);
    }
    if ctx.unit_enum(name).is_some() {
        // A unit enum crosses as its wire string, which the `.d.ts` narrows to
        // the union of that enum's literals. `crate::convert` owns both halves
        // of the mapping, including the refusal of a string outside the set.
        return quote!(::std::string::String);
    }
    if level == Level::Top {
        return quote!(::wasm_bindgen::JsValue);
    }
    let twin = convert::twin_ident(name);
    quote!(#twin)
}

/// Adapt a wrapper argument (typed by [`decl`]) to what the user's function
/// takes: reborrow a string or a byte string, rebuild a path from the string
/// JavaScript sent.
///
/// Everything whose *value* differs between the two sides is adapted by
/// [`crate::convert`] in a prologue statement instead; this is only the
/// reborrowing, so it cannot fail and needs no statement of its own.
pub fn pass(ty: &ir::Type, expr: &TokenStream) -> TokenStream {
    match ty {
        ir::Type::String { owned: false } => quote!(#expr.as_str()),
        ir::Type::Bytes { owned: false } => quote!(#expr.as_slice()),
        ir::Type::Path { owned: false } => quote!(::std::path::Path::new(#expr.as_str())),
        ir::Type::Path { owned: true } => quote!(::std::path::PathBuf::from(#expr)),
        ir::Type::Option(inner) => match &**inner {
            ir::Type::String { owned: false } | ir::Type::Bytes { owned: false } => {
                quote!(#expr.as_deref())
            }
            ir::Type::Path { owned: false } => {
                quote!(#expr.as_deref().map(::std::path::Path::new))
            }
            ir::Type::Path { owned: true } => {
                quote!(#expr.map(::std::path::PathBuf::from))
            }
            _ => quote!(#expr),
        },
        _ => quote!(#expr),
    }
}

/// How the user's return value reaches JavaScript.
///
/// Streams and the unit return are the caller's business ([`crate::function`]
/// names the per-export stream class); everything else is here.
pub fn returned(ty: &ir::Type, ctx: &TyCtx<'_>) -> Result<Returned, RenderError> {
    let declared = decl(ty, ctx, Level::Top)?;
    if !crosses_natively(ty, ctx) {
        let serde_ty = decl(ty, ctx, Level::Inner)?;
        let inner = convert::outward(ty, ctx, &quote!(value)).unwrap_or_else(|| quote!(value));
        return Ok(Returned {
            decl: declared,
            value: quote!(__unibind_wasm_to_js::<#serde_ty>(&#inner)),
            fallible: true,
        });
    }
    if let Some(value) = convert::path_outward(ty, &quote!(value)) {
        return Ok(Returned {
            decl: declared,
            value,
            fallible: true,
        });
    }
    let value = match ty {
        ir::Type::Named(name) => ctx.object(name).map_or_else(
            || convert::outward(ty, ctx, &quote!(value)).unwrap_or_else(|| quote!(value)),
            |object| {
                let handle = object_handle_ident(object);
                quote!(#handle::__unibind_from(value))
            },
        ),
        _ => convert::outward(ty, ctx, &quote!(value)).unwrap_or_else(|| quote!(value)),
    };
    Ok(Returned {
        decl: declared,
        value,
        fallible: false,
    })
}

/// The `JsValue` a `Promise` resolves with, from the value the sync
/// declaration hands back.
///
/// `None` when that value is already a `JsValue` -- everything crossing
/// through serde -- so the composition renders no identity `map`.
pub fn js_value(ty: Option<&ir::Type>, ctx: &TyCtx<'_>, expr: &TokenStream) -> Option<TokenStream> {
    let Some(ty) = ty else {
        return Some(quote!(::wasm_bindgen::JsValue::UNDEFINED));
    };
    if !matches!(ty, ir::Type::Stream(_)) && !crosses_natively(ty, ctx) {
        return None;
    }
    Some(match ty {
        // `JsValue::from` has no arm for a byte string, and going through
        // serde would spell it as an array of numbers; a `Uint8Array` view is
        // what the signature promised.
        ir::Type::Bytes { .. } => {
            quote!(::wasm_bindgen::JsValue::from(::js_sys::Uint8Array::from(&#expr[..])))
        }
        ir::Type::Option(inner) if matches!(**inner, ir::Type::Bytes { .. }) => quote! {
            #expr.map_or(
                ::wasm_bindgen::JsValue::NULL,
                |__unibind_bytes| {
                    ::wasm_bindgen::JsValue::from(
                        ::js_sys::Uint8Array::from(&__unibind_bytes[..]),
                    )
                },
            )
        },
        // `None` resolves as `null`, the same absent value the ts backend's
        // `Option` returns cross as.
        ir::Type::Option(_) => quote! {
            #expr.map_or(::wasm_bindgen::JsValue::NULL, ::wasm_bindgen::JsValue::from)
        },
        _ => quote!(::wasm_bindgen::JsValue::from(#expr)),
    })
}

/// The `Result<JsValue, JsValue>` expression one value settles into, written
/// against `value`. `settle` is `None` for a unit value, which resolves
/// `undefined`.
///
/// Shared by the `Promise`-returning wrappers and the stream classes: both hand
/// a `JsValue` to `future_to_promise`, and a value that reaches JavaScript one
/// way must reach it the same way the other.
pub fn resolved(
    ty: Option<&ir::Type>,
    ctx: &TyCtx<'_>,
    settle: Option<&Returned>,
) -> TokenStream {
    let Some(settle) = settle else {
        return quote!(::std::result::Result::Ok(::wasm_bindgen::JsValue::UNDEFINED));
    };
    let value = &settle.value;
    if settle.fallible {
        return js_value(ty, ctx, &quote!(__unibind_value)).map_or_else(
            // Already a `JsValue`: everything crossing through serde, whose move
            // is the fallible step itself.
            || quote!(#value.map_err(__unibind_wasm_error)),
            |js| {
                quote! {
                    #value
                        .map(|__unibind_value| #js)
                        .map_err(__unibind_wasm_error)
                }
            },
        );
    }
    let js = js_value(ty, ctx, value).unwrap_or_else(|| value.clone());
    quote!(::std::result::Result::Ok(#js))
}

/// The Rust identifier of the generated handle class for `object`.
pub fn object_handle_ident(object: &ir::Object) -> Ident {
    Ident::new(
        &format!("__UnibindWasmObject{}", object.name),
        Span::call_site(),
    )
}

/// The Rust identifier of the generated stream class for one stream-returning
/// export: `__UnibindWasmStreamTail` for a free `tail`,
/// `__UnibindWasmStreamCounterTail` for `Counter::tail`.
///
/// Export names are unique per scope (Rust enforces it), so classes cannot
/// collide within a scope; a free function named exactly like an
/// object+method concatenation would collide across scopes, and fails loudly
/// as a duplicate item in the glue module rather than silently misbinding.
pub fn stream_class_ident(owner: Option<&str>, export: &str) -> Ident {
    let export = pascal_case(export);
    let name = owner.map_or_else(
        || format!("__UnibindWasmStream{export}"),
        |object| format!("__UnibindWasmStream{object}{export}"),
    );
    Ident::new(&name, Span::call_site())
}

/// A 64-bit integer is declared `f64` -- a JavaScript `number` -- in both
/// directions; `crate::convert` owns the checked adaptation to and from the
/// user's own width (an inbound value that is fractional or outside the safe
/// integer range is refused, never truncated). Everything narrower crosses as
/// its own Rust type, which `wasm-bindgen` carries as a `number` natively.
fn int_tokens(kind: ir::IntKind) -> TokenStream {
    if convert::is_wide_int(kind) {
        return quote!(f64);
    }
    let ident = Ident::new(kind.rust_name(), Span::call_site());
    quote!(#ident)
}
