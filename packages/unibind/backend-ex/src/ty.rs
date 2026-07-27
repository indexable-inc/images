//! Render IR types back into Rust token streams for the generated glue,
//! and validate that a type is representable on the BEAM boundary.
//!
//! One type does not cross as itself: binary payloads are spelled
//! [`unibind_ex_runtime::Bytes`] in the wrapper signature, because
//! rustler's own `Vec<u8>` codec is the element-wise `Vec<T>` one and puts
//! a payload on the BEAM as a list of integers. The wrappers convert at
//! the call site ([`forward`] inbound, [`to_wire`] outbound), so the
//! user's own function keeps its `&[u8]` / `Vec<u8>` signature.

use proc_macro2::{Ident, Span, TokenStream};
use quote::quote;
use unibind_core::ir;
use unibind_core::render::{self, Ownership, RenderError};

/// The Rust spelling of a boundary type, as the wrapper signatures use it.
/// `user` is the exported module's identifier; named types resolve through
/// `super::<user>::`.
///
/// Rejects the types rustler cannot carry: see [`check_boundary`].
pub fn rust_type(ty: &ir::Type, user: &Ident) -> Result<TokenStream, RenderError> {
    check_boundary(ty)?;
    Ok(wire_type(ty, user, Ownership::Declared))
}

/// Like [`rust_type`], but with every borrow owned: async wrappers move
/// their arguments into a `'static` future, so `&str` arrives as `String`
/// and is re-borrowed at the call site.
pub fn owned_type(ty: &ir::Type, user: &Ident) -> Result<TokenStream, RenderError> {
    check_boundary(ty)?;
    Ok(wire_type(ty, user, Ownership::Owned))
}

/// The wire spelling: [`render::rust_type`], except that binary payloads
/// become the `Bytes` newtype at every depth. A borrowed `&[u8]` has no
/// borrowed wire form -- the newtype owns the copy the decoder made -- so
/// both ownerships spell it the same and [`forward`] borrows back out.
fn wire_type(ty: &ir::Type, user: &Ident, ownership: Ownership) -> TokenStream {
    if !contains_bytes(ty) {
        return render::rust_type(ty, user, ownership);
    }
    match ty {
        ir::Type::Bytes { .. } => quote!(::unibind_ex_runtime::Bytes),
        ir::Type::Option(inner) => {
            let inner = wire_type(inner, user, ownership);
            quote!(::std::option::Option<#inner>)
        }
        ir::Type::Vec(inner) => {
            let inner = wire_type(inner, user, ownership);
            quote!(::std::vec::Vec<#inner>)
        }
        ir::Type::Map { key, value } => {
            let key = wire_type(key, user, ownership);
            let value = wire_type(value, user, ownership);
            quote!(::std::collections::HashMap<#key, #value>)
        }
        ir::Type::Stream(item) => {
            let item = wire_type(item, user, ownership);
            quote!(::unibind_runtime::UniStream<#item>)
        }
        // `contains_bytes` ruled out every leaf that is not `Bytes`.
        ir::Type::Bool
        | ir::Type::Int(_)
        | ir::Type::Float(_)
        | ir::Type::String { .. }
        | ir::Type::Path { .. }
        | ir::Type::Named(_) => render::rust_type(ty, user, ownership),
    }
}

/// Whether a type carries binary data anywhere inside it, and so needs the
/// wire newtype plus a conversion at the call site.
///
/// Records never do: [`crate::record`] rejects byte fields outright,
/// because a record's Elixir codec is a `NifStruct` derive spliced onto
/// the user's own struct and has no call site to convert at.
pub fn contains_bytes(ty: &ir::Type) -> bool {
    match ty {
        ir::Type::Bytes { .. } => true,
        ir::Type::Option(inner) | ir::Type::Vec(inner) | ir::Type::Stream(inner) => {
            contains_bytes(inner)
        }
        ir::Type::Map { key, value } => contains_bytes(key) || contains_bytes(value),
        ir::Type::Bool
        | ir::Type::Int(_)
        | ir::Type::Float(_)
        | ir::Type::String { .. }
        | ir::Type::Path { .. }
        | ir::Type::Named(_) => false,
    }
}

/// The call-site expression forwarding a wrapper argument to the user's
/// function.
///
/// `ownership` is how the parameter was spelled: an async wrapper owns its
/// arguments (a declared `&str` arrived as `String`) and re-borrows here.
/// Binary arguments arrive as the owned wire newtype under either
/// ownership, so both borrow out of it the same way.
pub fn forward(name: &Ident, ty: &ir::Type, ownership: Ownership) -> TokenStream {
    let reborrows = matches!(ownership, Ownership::Owned);
    match ty {
        // The wire value owns the decoder's copy; the borrow points into
        // the wrapper's own binding, which outlives the call.
        ir::Type::Bytes { owned: false } => quote!(&#name.0),
        ir::Type::String { owned: false } | ir::Type::Path { owned: false } if reborrows => {
            quote!(&#name)
        }
        ir::Type::Option(inner) => match &**inner {
            ir::Type::Bytes { owned: false } => {
                quote!(#name.as_ref().map(|bytes| bytes.0.as_slice()))
            }
            ir::Type::String { owned: false } | ir::Type::Path { owned: false } if reborrows => {
                quote!(#name.as_deref())
            }
            _ => from_wire(name, ty),
        },
        _ => from_wire(name, ty),
    }
}

/// Convert an owned wire binding into the user's declared type.
///
/// Only owned forms reach here. The IR allows a borrow at the top of an
/// argument and directly under `Option` (`lower::ty::Position`), and
/// [`forward`] handles both before recursing.
fn from_wire(name: &Ident, ty: &ir::Type) -> TokenStream {
    if !contains_bytes(ty) {
        return quote!(#name);
    }
    match ty {
        ir::Type::Bytes { .. } => quote!(#name.0),
        ir::Type::Option(inner) => {
            let item = item_ident();
            let inner = from_wire(&item, inner);
            quote!(#name.map(|#item| #inner))
        }
        ir::Type::Vec(inner) => {
            let item = item_ident();
            let inner = from_wire(&item, inner);
            quote!(#name.into_iter().map(|#item| #inner).collect())
        }
        ir::Type::Map { key, value } => {
            let key_ident = key_ident();
            let item = item_ident();
            let key = from_wire(&key_ident, key);
            let value = from_wire(&item, value);
            quote!(#name.into_iter().map(|(#key_ident, #item)| (#key, #value)).collect())
        }
        _ => quote!(#name),
    }
}

/// Convert a value of the user's type into its wire spelling, so rustler
/// encodes binaries as binaries.
///
/// The identity when the type carries no bytes, which keeps every existing
/// rendering byte for byte unchanged.
pub fn to_wire(expr: &TokenStream, ty: &ir::Type) -> TokenStream {
    if !contains_bytes(ty) {
        return expr.clone();
    }
    match ty {
        ir::Type::Bytes { .. } => quote!(::unibind_ex_runtime::Bytes(#expr)),
        ir::Type::Option(inner) => {
            let item = item_ident();
            let inner = to_wire(&quote!(#item), inner);
            quote!(#expr.map(|#item| #inner))
        }
        ir::Type::Vec(inner) => {
            let item = item_ident();
            let inner = to_wire(&quote!(#item), inner);
            quote!(#expr.into_iter().map(|#item| #inner).collect())
        }
        ir::Type::Map { key, value } => {
            let key_ident = key_ident();
            let item = item_ident();
            let key = to_wire(&quote!(#key_ident), key);
            let value = to_wire(&quote!(#item), value);
            quote!(#expr.into_iter().map(|(#key_ident, #item)| (#key, #value)).collect())
        }
        ir::Type::Stream(item) => to_wire_stream(expr, item),
        _ => expr.clone(),
    }
}

/// A stream expression with its items converted to wire types.
///
/// Streams never appear in a wrapper signature -- they are handed straight
/// to the runtime, which encodes one item per granted credit -- so the
/// conversion re-wraps the stream rather than the value.
pub fn to_wire_stream(expr: &TokenStream, item: &ir::Type) -> TokenStream {
    if !contains_bytes(item) {
        return expr.clone();
    }
    let value = item_ident();
    let wired = to_wire(&quote!(#value), item);
    quote!(::unibind_ex_runtime::map_stream(#expr, |#value| #wired))
}

/// A call expression with its return value converted to wire types.
pub fn to_wire_value(call: &TokenStream, ret: Option<&ir::Type>) -> TokenStream {
    ret.map_or_else(|| call.clone(), |ret| to_wire(call, ret))
}

/// Like [`to_wire_value`], for a call that returns a `Result`: the value
/// rides inside `Ok`, so the conversion maps through it.
pub fn to_wire_ok(call: &TokenStream, ret: Option<&ir::Type>) -> TokenStream {
    let Some(ret) = ret else {
        return call.clone();
    };
    if !contains_bytes(ret) {
        return call.clone();
    }
    let value = item_ident();
    let wired = to_wire(&quote!(#value), ret);
    quote!(#call.map(|#value| #wired))
}

fn item_ident() -> Ident {
    Ident::new("value", Span::call_site())
}

fn key_ident() -> Ident {
    Ident::new("key", Span::call_site())
}

/// Reject the types the elixir backend cannot carry across the boundary:
/// nested streams (`Stream<T>` only crosses as a whole function return).
///
/// Binaries used to be rejected here; they now cross through [`wire_type`]'s
/// newtype.
pub fn check_boundary(ty: &ir::Type) -> Result<(), RenderError> {
    match ty {
        ir::Type::Stream(_) => Err(RenderError::new(
            "`Stream<T>` only crosses as the whole return type of a stream \
             function",
        )),
        ir::Type::Option(inner) | ir::Type::Vec(inner) => check_boundary(inner),
        ir::Type::Map { key, value } => {
            check_boundary(key)?;
            check_boundary(value)
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
