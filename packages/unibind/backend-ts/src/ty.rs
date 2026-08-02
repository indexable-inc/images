//! Render IR types into the Rust tokens of the generated napi glue.

use proc_macro2::{Ident, Span, TokenStream};
use quote::quote;
use unibind_core::ir;
use unibind_core::render::{RenderError, name_ident, pascal_case};

use crate::convert;

/// Interface-wide context the type mapping needs: the alias the glue binds
/// to the exported module (named types resolve through `<user>::`), the
/// declared objects (which map to generated handle classes, not user
/// structs), and the records that cross through a generated mirror struct.
pub struct TyCtx<'a> {
    /// The glue-level alias for the user's module, never `super::<module>`
    /// directly: see the binding in [`crate::module`] for why the hop
    /// cannot survive napi-derive's helper modules.
    pub user: &'a Ident,
    pub objects: &'a [ir::Object],
    pub mirrored: &'a [String],
}

impl TyCtx<'_> {
    pub fn object(&self, name: &str) -> Option<&ir::Object> {
        self.objects.iter().find(|object| object.name == name)
    }

    /// The mirror struct standing in for record `name`, when it has one
    /// (see [`crate::mirror`]).
    pub fn mirror(&self, name: &str) -> Option<Ident> {
        self.mirrored
            .iter()
            .any(|mirrored| mirrored == name)
            .then(|| convert::mirror_ident(name))
    }
}

/// Which position a value occupies, which is what decides whether bytes
/// cross as napi's `Buffer` or as the user's own `Vec<u8>`.
///
/// The dividing line is not depth: it is whether the glue writes a
/// conversion of its own for this value. A whole argument or return value
/// gets one, and so does every field of a record's generated mirror struct
/// (a `Buffer` is a legal `#[napi(object)]` field -- it implements both
/// napi conversions). A `Vec` element or a map value gets none: the
/// container crosses whole and unconverted, so its interior has to already
/// be the type the user's own code spells.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Level {
    /// A whole argument, return value, or stream item.
    Top,
    /// A field of a record (see [`crate::mirror`]).
    Field,
    /// Inside a container: a `Vec` element or a map value.
    Element,
}

impl Level {
    /// Whether bytes at this position cross as napi's `Buffer`. Every
    /// renderer asks here rather than matching the variants, so the
    /// generated glue, the `.d.ts` and the Zod schema cannot disagree about
    /// one position.
    pub const fn bytes_as_buffer(self) -> bool {
        match self {
            Self::Top | Self::Field => true,
            Self::Element => false,
        }
    }
}

/// Reject the type surface napi cannot represent faithfully under the
/// pinned feature set. Walks nested types; run it before spelling any
/// tokens so failures name the follow-up instead of miscompiling.
pub fn check(ty: &ir::Type, what: &str) -> Result<(), RenderError> {
    match ty {
        ir::Type::Map { key, value } => {
            if !matches!(**key, ir::Type::String { .. }) {
                return Err(RenderError::new(format!(
                    "{what} is a map with non-string keys; JavaScript object \
                     keys are strings, so integer-keyed maps are not part of \
                     the ts backend yet (issue #1993)"
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

/// The Rust type a `#[napi]` wrapper declares for a value crossing at
/// `level`. Streams and objects never reach here in argument position (the
/// lowering confines them to whole return types).
pub fn decl(ty: &ir::Type, ctx: &TyCtx<'_>, level: Level) -> Result<TokenStream, RenderError> {
    Ok(match ty {
        ir::Type::Bool => quote!(bool),
        ir::Type::Int(kind) => int_tokens(*kind),
        ir::Type::Float(ir::FloatKind::F32) => quote!(f32),
        ir::Type::Float(ir::FloatKind::F64) => quote!(f64),
        ir::Type::String { .. } => quote!(::std::string::String),
        ir::Type::Path { .. } => quote!(::std::path::PathBuf),
        ir::Type::Bytes { .. } => {
            if level.bytes_as_buffer() {
                quote!(::napi::bindgen_prelude::Buffer)
            } else {
                quote!(::std::vec::Vec<u8>)
            }
        }
        ir::Type::Option(inner) => {
            let inner = decl(inner, ctx, level)?;
            quote!(::std::option::Option<#inner>)
        }
        ir::Type::Vec(inner) => {
            let inner = decl(inner, ctx, Level::Element)?;
            quote!(::std::vec::Vec<#inner>)
        }
        ir::Type::Map { value, .. } => {
            let value = decl(value, ctx, Level::Element)?;
            quote!(::std::collections::HashMap<::std::string::String, #value>)
        }
        ir::Type::Named(name) => {
            if let Some(object) = ctx.object(name) {
                let handle = object_handle_ident(object);
                quote!(#handle)
            } else if let Some(mirror) = ctx.mirror(name) {
                quote!(#mirror)
            } else {
                let user = ctx.user;
                let name = name_ident(name)?;
                quote!(#user::#name)
            }
        }
        ir::Type::Stream(_) => {
            return Err(RenderError::new(
                "streams cross only as a whole return type; the wrapper never \
                 spells UniStream in a napi signature"
                    .to_owned(),
            ));
        }
    })
}

/// Adapt a wrapper argument (typed by [`decl`]) to what the user's function
/// takes: reborrow borrowed forms, unwrap `Buffer` into `Vec<u8>`.
pub fn pass(ty: &ir::Type, expr: &TokenStream) -> TokenStream {
    match ty {
        ir::Type::String { owned: false } => quote!(#expr.as_str()),
        ir::Type::Path { owned: false } => quote!(#expr.as_path()),
        ir::Type::Bytes { owned: false } => quote!(#expr.as_ref()),
        ir::Type::Bytes { owned: true } => quote!(::std::vec::Vec::from(#expr)),
        ir::Type::Option(inner) => match &**inner {
            ir::Type::String { owned: false }
            | ir::Type::Path { owned: false }
            | ir::Type::Bytes { owned: false } => quote!(#expr.as_deref()),
            ir::Type::Bytes { owned: true } => quote!(#expr.map(::std::vec::Vec::from)),
            _ => quote!(#expr),
        },
        _ => quote!(#expr),
    }
}

/// Adapt the user's return value to the wrapper's declared return type:
/// widen 64-bit integers (and the records holding them) into their `BigInt`
/// shape, wrap bytes into `Buffer`, wrap constructed objects into their
/// handle.
pub fn ret(ty: &ir::Type, ctx: &TyCtx<'_>, expr: &TokenStream) -> TokenStream {
    if let Some(converted) = convert::outward(ty, ctx, expr) {
        return converted;
    }
    match ty {
        ir::Type::Bytes { .. } => quote!(::napi::bindgen_prelude::Buffer::from(#expr)),
        ir::Type::Option(inner) if matches!(**inner, ir::Type::Bytes { .. }) => {
            quote!(#expr.map(::napi::bindgen_prelude::Buffer::from))
        }
        ir::Type::Named(name) => ctx.object(name).map_or_else(
            || quote!(#expr),
            |object| {
                let handle = object_handle_ident(object);
                quote!(#handle::__unibind_from(#expr))
            },
        ),
        _ => quote!(#expr),
    }
}

/// The Rust identifier of the generated handle class for `object`.
pub fn object_handle_ident(object: &ir::Object) -> Ident {
    Ident::new(
        &format!("__UnibindObject{}", object.name),
        Span::call_site(),
    )
}

/// The Rust identifier of the generated stream class for one
/// stream-returning export: `__UnibindStreamTail` for a free `tail`,
/// `__UnibindStreamCounterTail` for `Counter::tail`.
///
/// Export names are unique per scope (Rust enforces it), so classes cannot
/// collide within a scope; a free function named exactly like an
/// object+method concatenation would collide across scopes, and fails
/// loudly as a duplicate item in the glue module rather than silently
/// misbinding.
pub fn stream_class_ident(owner: Option<&str>, export: &str) -> Ident {
    let export = pascal_case(export);
    let name = owner.map_or_else(
        || format!("__UnibindStream{export}"),
        |object| format!("__UnibindStream{object}{export}"),
    );
    Ident::new(&name, Span::call_site())
}

/// A 64-bit integer is declared as napi's `BigInt` in both directions;
/// `crate::convert` owns the adaptation to and from the user's own width.
/// Everything narrower crosses as its own Rust type, which napi carries as
/// a JavaScript `number`.
fn int_tokens(kind: ir::IntKind) -> TokenStream {
    if convert::is_bigint(kind) {
        return quote!(::napi::bindgen_prelude::BigInt);
    }
    let ident = Ident::new(kind.rust_name(), Span::call_site());
    quote!(#ident)
}
