//! Lower syn types into [`crate::ir::Type`].

use quote::ToTokens as _;
use syn::spanned::Spanned as _;

use super::{Declared, LowerError, Result};
use crate::ir;

/// Where a type appears; borrowed forms are only legal in argument position.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Position {
    /// A function argument (borrowed `&str` / `&Path` / `&[u8]` allowed,
    /// also directly under `Option`).
    Arg,
    /// A return type or record field: owned types only.
    Owned,
}

pub fn lower_type(ty: &syn::Type, declared: &Declared, position: Position) -> Result<ir::Type> {
    match ty {
        syn::Type::Reference(reference) => lower_reference(reference, position),
        syn::Type::Path(path) => lower_path(path, declared, position),
        syn::Type::Paren(inner) => lower_type(&inner.elem, declared, position),
        _ => Err(unsupported(ty)),
    }
}

fn lower_reference(reference: &syn::TypeReference, position: Position) -> Result<ir::Type> {
    if position != Position::Arg {
        return Err(LowerError::new(
            reference.span(),
            "borrowed types only work as argument types; own the data \
             (String, PathBuf, Vec<u8>) everywhere else",
        ));
    }
    if reference.mutability.is_some() {
        return Err(LowerError::new(
            reference.span(),
            "&mut does not cross the binding boundary; return the new value instead",
        ));
    }
    match &*reference.elem {
        syn::Type::Path(path) if path.qself.is_none() && path.path.is_ident("str") => {
            Ok(ir::Type::String { owned: false })
        }
        syn::Type::Path(path)
            if path.qself.is_none() && last_ident(&path.path).is_some_and(|i| i == "Path") =>
        {
            Ok(ir::Type::Path { owned: false })
        }
        syn::Type::Slice(slice) => match &*slice.elem {
            syn::Type::Path(path) if path.path.is_ident("u8") => {
                Ok(ir::Type::Bytes { owned: false })
            }
            _ => Err(LowerError::new(
                slice.span(),
                "only &[u8] slices cross the boundary; use Vec<T> for lists",
            )),
        },
        _ => Err(LowerError::new(
            reference.span(),
            "unsupported borrowed type; pass &str, &Path, or &[u8]",
        )),
    }
}

fn lower_path(path: &syn::TypePath, declared: &Declared, position: Position) -> Result<ir::Type> {
    if path.qself.is_some() {
        return Err(unsupported(&syn::Type::Path(path.clone())));
    }
    let Some(segment) = path.path.segments.last() else {
        return Err(unsupported(&syn::Type::Path(path.clone())));
    };
    let ident = segment.ident.to_string();
    if let Some(kind) = int_kind(&ident) {
        return no_generics(segment).map(|()| ir::Type::Int(kind));
    }
    match ident.as_str() {
        "bool" => no_generics(segment).map(|()| ir::Type::Bool),
        "f32" => no_generics(segment).map(|()| ir::Type::Float(ir::FloatKind::F32)),
        "f64" => no_generics(segment).map(|()| ir::Type::Float(ir::FloatKind::F64)),
        "String" => no_generics(segment).map(|()| ir::Type::String { owned: true }),
        "PathBuf" => no_generics(segment).map(|()| ir::Type::Path { owned: true }),
        "Option" => {
            let inner = one_generic(segment)?;
            Ok(ir::Type::Option(Box::new(lower_type(inner, declared, position)?)))
        }
        "Vec" => {
            let inner = one_generic(segment)?;
            if let syn::Type::Path(inner_path) = inner
                && inner_path.path.is_ident("u8")
            {
                return Ok(ir::Type::Bytes { owned: true });
            }
            Ok(ir::Type::Vec(Box::new(lower_type(
                inner,
                declared,
                Position::Owned,
            )?)))
        }
        "HashMap" => {
            let pair = two_generics(segment)?;
            let key = lower_type(pair.key, declared, Position::Owned)?;
            if !matches!(key, ir::Type::String { .. } | ir::Type::Int(_)) {
                return Err(LowerError::new(
                    segment.span(),
                    "map keys are strings or integers in phase 0",
                ));
            }
            Ok(ir::Type::Map {
                key: Box::new(key),
                value: Box::new(lower_type(pair.value, declared, Position::Owned)?),
            })
        }
        _ => lower_named(path, segment, &ident, declared),
    }
}

fn lower_named(
    path: &syn::TypePath,
    segment: &syn::PathSegment,
    ident: &str,
    declared: &Declared,
) -> Result<ir::Type> {
    no_generics(segment)?;
    if path.path.segments.len() != 1 {
        return Err(unsupported(&syn::Type::Path(path.clone())));
    }
    if declared.records.iter().any(|name| name == ident) {
        return Ok(ir::Type::Named(ident.to_owned()));
    }
    if declared.errors.iter().any(|name| name == ident) {
        return Err(LowerError::new(
            segment.span(),
            "an error enum crosses the boundary only through Result",
        ));
    }
    Err(LowerError::new(
        segment.span(),
        format!(
            "`{ident}` is not a #[unibind::record] in this module; only records \
             and boundary primitives cross"
        ),
    ))
}

fn int_kind(ident: &str) -> Option<ir::IntKind> {
    Some(match ident {
        "i8" => ir::IntKind::I8,
        "i16" => ir::IntKind::I16,
        "i32" => ir::IntKind::I32,
        "i64" => ir::IntKind::I64,
        "isize" => ir::IntKind::Isize,
        "u8" => ir::IntKind::U8,
        "u16" => ir::IntKind::U16,
        "u32" => ir::IntKind::U32,
        "u64" => ir::IntKind::U64,
        "usize" => ir::IntKind::Usize,
        _ => return None,
    })
}

fn last_ident(path: &syn::Path) -> Option<&syn::Ident> {
    path.segments.last().map(|segment| &segment.ident)
}

fn no_generics(segment: &syn::PathSegment) -> Result<()> {
    if matches!(segment.arguments, syn::PathArguments::None) {
        Ok(())
    } else {
        Err(LowerError::new(
            segment.span(),
            "unexpected generic arguments on this type",
        ))
    }
}

fn generic_types(segment: &syn::PathSegment) -> Result<Vec<&syn::Type>> {
    let syn::PathArguments::AngleBracketed(arguments) = &segment.arguments else {
        return Err(LowerError::new(
            segment.span(),
            "this container needs angle-bracketed type arguments",
        ));
    };
    Ok(arguments
        .args
        .iter()
        .filter_map(|argument| match argument {
            syn::GenericArgument::Type(ty) => Some(ty),
            _ => None,
        })
        .collect())
}

fn one_generic(segment: &syn::PathSegment) -> Result<&syn::Type> {
    let types = generic_types(segment)?;
    if let [inner] = types.as_slice() {
        Ok(inner)
    } else {
        Err(LowerError::new(
            segment.span(),
            "expected exactly one type argument",
        ))
    }
}

/// The `(key, value)` pair of a two-argument container.
struct KeyValue<'a> {
    key: &'a syn::Type,
    value: &'a syn::Type,
}

fn two_generics(segment: &syn::PathSegment) -> Result<KeyValue<'_>> {
    let types = generic_types(segment)?;
    if let [key, value] = types.as_slice() {
        Ok(KeyValue { key, value })
    } else {
        Err(LowerError::new(
            segment.span(),
            "expected exactly two type arguments",
        ))
    }
}

fn unsupported(ty: &syn::Type) -> LowerError {
    LowerError::new(
        ty.span(),
        format!(
            "unsupported type `{}` at the unibind boundary",
            ty.to_token_stream()
        ),
    )
}
