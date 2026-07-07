//! Render IR types into the Rust tokens of the generated napi glue.

use proc_macro2::{Ident, Span, TokenStream};
use quote::quote;
use unibind_core::ir;

use crate::RenderError;

/// Interface-wide context the type mapping needs: the exported module's
/// identifier (named types resolve through `super::<user>::`) and the
/// declared objects (which map to generated handle classes, not user
/// structs).
pub struct TyCtx<'a> {
    pub user: &'a Ident,
    pub objects: &'a [ir::Object],
}

impl TyCtx<'_> {
    pub fn object(&self, name: &str) -> Option<&ir::Object> {
        self.objects.iter().find(|object| object.name == name)
    }
}

/// How close to the wrapper signature a type sits. `Buffer` only replaces
/// bytes at the top level (including directly under `Option`); nested bytes
/// stay `Vec<u8>` so container types line up with the user's own field and
/// element types.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Level {
    Top,
    Nested,
}

/// Reject the type surface napi cannot represent faithfully under the
/// pinned feature set. Walks nested types; run it before spelling any
/// tokens so failures name the follow-up instead of miscompiling.
pub fn check(ty: &ir::Type, what: &str) -> Result<(), RenderError> {
    match ty {
        ir::Type::Int(kind) => match kind {
            ir::IntKind::U64 | ir::IntKind::Usize | ir::IntKind::Isize => {
                Err(RenderError::new(format!(
                    "{what} is `{}`, which napi only carries as a BigInt; \
                     BigInt mapping is a stage 2 follow-up of issue #1993, so \
                     use i64 (IEEE-double safe range) or u32 for now",
                    int_name(*kind),
                )))
            }
            _ => Ok(()),
        },
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
        ir::Type::Bytes { .. } => match level {
            Level::Top => quote!(::napi::bindgen_prelude::Buffer),
            Level::Nested => quote!(::std::vec::Vec<u8>),
        },
        ir::Type::Option(inner) => {
            let inner = decl(inner, ctx, level)?;
            quote!(::std::option::Option<#inner>)
        }
        ir::Type::Vec(inner) => {
            let inner = decl(inner, ctx, Level::Nested)?;
            quote!(::std::vec::Vec<#inner>)
        }
        ir::Type::Map { value, .. } => {
            let value = decl(value, ctx, Level::Nested)?;
            quote!(::std::collections::HashMap<::std::string::String, #value>)
        }
        ir::Type::Named(name) => {
            if let Some(object) = ctx.object(name) {
                let handle = object_handle_ident(object);
                quote!(#handle)
            } else {
                let user = ctx.user;
                let name = name_ident(name)?;
                quote!(super::#user::#name)
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
/// wrap bytes into `Buffer`, wrap constructed objects into their handle.
pub fn ret(ty: &ir::Type, ctx: &TyCtx<'_>, expr: &TokenStream) -> TokenStream {
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

/// The Rust identifier of the generated stream class for the function
/// `name`.
pub fn stream_class_ident(name: &str) -> Ident {
    Ident::new(
        &format!("__UnibindStream{}", pascal_case(name)),
        Span::call_site(),
    )
}

/// `snake_case` or `camelCase` to `PascalCase`, for generated class names.
pub fn pascal_case(name: &str) -> String {
    name.split('_')
        .filter(|segment| !segment.is_empty())
        .map(|segment| {
            let mut chars = segment.chars();
            chars.next().map_or_else(String::new, |first| {
                first.to_ascii_uppercase().to_string() + chars.as_str()
            })
        })
        .collect()
}

/// An identifier for a possibly-keyword name (renames like `type` fall back
/// to raw identifiers).
pub fn name_ident(name: &str) -> Result<Ident, RenderError> {
    syn::parse_str::<Ident>(name)
        .or_else(|_| syn::parse_str::<Ident>(&format!("r#{name}")))
        .map_err(|_| RenderError::new(format!("`{name}` is not usable as an identifier")))
}

fn int_tokens(kind: ir::IntKind) -> TokenStream {
    match kind {
        ir::IntKind::I8 => quote!(i8),
        ir::IntKind::I16 => quote!(i16),
        ir::IntKind::I32 => quote!(i32),
        ir::IntKind::I64 => quote!(i64),
        ir::IntKind::U8 => quote!(u8),
        ir::IntKind::U16 => quote!(u16),
        ir::IntKind::U32 => quote!(u32),
        // Rejected by `check` before any of these spell into a signature.
        ir::IntKind::U64 | ir::IntKind::Usize | ir::IntKind::Isize => quote!(::core::compile_error!(
            "unreachable: BigInt-only integers are rejected at render time"
        )),
    }
}

const fn int_name(kind: ir::IntKind) -> &'static str {
    match kind {
        ir::IntKind::I8 => "i8",
        ir::IntKind::I16 => "i16",
        ir::IntKind::I32 => "i32",
        ir::IntKind::I64 => "i64",
        ir::IntKind::Isize => "isize",
        ir::IntKind::U8 => "u8",
        ir::IntKind::U16 => "u16",
        ir::IntKind::U32 => "u32",
        ir::IntKind::U64 => "u64",
        ir::IntKind::Usize => "usize",
    }
}
