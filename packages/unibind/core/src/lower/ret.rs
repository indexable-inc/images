//! Lower return positions: plain types, `Result`, streams, and
//! constructor returns.

use syn::spanned::Spanned as _;

use super::ty::{lower_type, one_generic, Position};
use super::{Declared, LowerError, Result};
use crate::ir;

/// A lowered return position: the success type and the thrown error, if any.
pub(super) struct Returned {
    pub(super) ty: Option<ir::Type>,
    pub(super) throws: Option<String>,
}

pub(super) fn lower_return(output: &syn::ReturnType, declared: &Declared) -> Result<Returned> {
    let syn::ReturnType::Type(_, ty) = output else {
        return Ok(Returned {
            ty: None,
            throws: None,
        });
    };
    if is_unit(ty) {
        return Ok(Returned {
            ty: None,
            throws: None,
        });
    }
    if let Some(segment) = stream_segment(ty) {
        return Ok(Returned {
            ty: Some(lower_stream(segment, declared)?),
            throws: None,
        });
    }
    if let syn::Type::Path(path) = &**ty
        && let Some(segment) = path.path.segments.last()
        && segment.ident == "Result"
    {
        return lower_result(segment, declared);
    }
    Ok(Returned {
        ty: Some(lower_type(ty, declared, Position::Return)?),
        throws: None,
    })
}

fn lower_result(segment: &syn::PathSegment, declared: &Declared) -> Result<Returned> {
    let parts = result_parts(segment)?;
    let throws = error_name(parts.error, declared)?;
    let ty = if is_unit(parts.ok) {
        None
    } else if let Some(stream) = stream_segment(parts.ok) {
        Some(lower_stream(stream, declared)?)
    } else {
        Some(lower_type(parts.ok, declared, Position::Return)?)
    };
    Ok(Returned {
        ty,
        throws: Some(throws),
    })
}

/// A constructor returns the object (or `Result` of it): the IR leaves
/// `ret` empty because the object itself is implied.
pub(super) fn lower_ctor_return(
    output: &syn::ReturnType,
    object: &str,
    declared: &Declared,
) -> Result<Returned> {
    let syn::ReturnType::Type(_, ty) = output else {
        return Err(bad_ctor(output.span(), object));
    };
    if is_object(ty, object) {
        return Ok(Returned {
            ty: None,
            throws: None,
        });
    }
    if let syn::Type::Path(path) = &**ty
        && let Some(segment) = path.path.segments.last()
        && segment.ident == "Result"
    {
        let parts = result_parts(segment)?;
        if !is_object(parts.ok, object) {
            return Err(bad_ctor(parts.ok.span(), object));
        }
        let throws = error_name(parts.error, declared)?;
        return Ok(Returned {
            ty: None,
            throws: Some(throws),
        });
    }
    Err(bad_ctor(ty.span(), object))
}

/// The `(ok, error)` types of a spelled-out `Result`.
struct OkErr<'a> {
    ok: &'a syn::Type,
    error: &'a syn::Type,
}

fn result_parts(segment: &syn::PathSegment) -> Result<OkErr<'_>> {
    let syn::PathArguments::AngleBracketed(arguments) = &segment.arguments else {
        return Err(LowerError::new(
            segment.span(),
            "spell the Result out as Result<T, YourError>",
        ));
    };
    let types: Vec<&syn::Type> = arguments
        .args
        .iter()
        .filter_map(|argument| match argument {
            syn::GenericArgument::Type(ty) => Some(ty),
            _ => None,
        })
        .collect();
    let [ok, error]: [&syn::Type; 2] = types.as_slice().try_into().map_err(|_| {
        LowerError::new(
            segment.span(),
            "spell the Result out as Result<T, YourError>; type aliases hide \
             the error type from the macro",
        )
    })?;
    Ok(OkErr { ok, error })
}

fn error_name(error: &syn::Type, declared: &Declared) -> Result<String> {
    let syn::Type::Path(error_path) = error else {
        return Err(bad_error_type(error));
    };
    let Some(error_ident) = error_path.path.get_ident() else {
        return Err(bad_error_type(error));
    };
    if !declared
        .errors
        .iter()
        .any(|name| error_ident == name.as_str())
    {
        return Err(bad_error_type(error));
    }
    Ok(error_ident.to_string())
}

/// The `UniStream` path segment of a type, when it names a stream.
pub(super) fn stream_segment(ty: &syn::Type) -> Option<&syn::PathSegment> {
    let syn::Type::Path(path) = ty else {
        return None;
    };
    if path.qself.is_some() {
        return None;
    }
    let segment = path.path.segments.last()?;
    (segment.ident == "UniStream").then_some(segment)
}

/// Lower the item of a `UniStream<T>` return into [`ir::Type::Stream`].
fn lower_stream(segment: &syn::PathSegment, declared: &Declared) -> Result<ir::Type> {
    let item = one_generic(segment)?;
    if let Some(nested) = stream_segment(item) {
        return Err(LowerError::new(
            nested.span(),
            "streams do not nest; yield the inner stream's items directly",
        ));
    }
    if let syn::Type::Path(path) = item
        && let Some(ident) = path.path.get_ident()
        && declared.objects.iter().any(|name| ident == name.as_str())
    {
        return Err(LowerError::new(
            item.span(),
            "streams of objects do not cross the boundary yet; \
             stream a #[unibind::record] snapshot instead",
        ));
    }
    Ok(ir::Type::Stream(Box::new(lower_type(
        item,
        declared,
        Position::Owned,
    )?)))
}


/// `Self` or the object's own name, as a bare path.
fn is_object(ty: &syn::Type, object: &str) -> bool {
    let syn::Type::Path(path) = ty else {
        return false;
    };
    path.qself.is_none()
        && path
            .path
            .get_ident()
            .is_some_and(|ident| ident == "Self" || ident == object)
}

fn bad_ctor(span: proc_macro2::Span, object: &str) -> LowerError {
    LowerError::new(
        span,
        format!(
            "a constructor returns `Self` (or `Result<Self, YourError>`) so \
             Python gets a `{object}`; move other signatures to methods or \
             free functions"
        ),
    )
}

fn bad_error_type(ty: &syn::Type) -> LowerError {
    LowerError::new(
        ty.span(),
        "the error type of an exported function must be a #[unibind::error] \
         enum declared in the same module",
    )
}

fn is_unit(ty: &syn::Type) -> bool {
    matches!(ty, syn::Type::Tuple(tuple) if tuple.elems.is_empty())
}
