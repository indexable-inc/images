//! Lower exported functions.

use syn::spanned::Spanned as _;

use super::ty::{lower_type, Position};
use super::{attrs, Declared, LowerError, Result};
use crate::ir;

pub(super) fn lower_fn(func: &syn::ItemFn, declared: &Declared) -> Result<ir::Function> {
    let signature = &func.sig;
    if let Some(asyncness) = signature.asyncness {
        return Err(LowerError::new(
            asyncness.span(),
            "async functions land in phase 2 (issue #1992); phase 0 exports \
             sync functions only",
        ));
    }
    if let Some(unsafety) = signature.unsafety {
        return Err(LowerError::new(
            unsafety.span(),
            "unsafe functions do not cross the binding boundary",
        ));
    }
    if !signature.generics.params.is_empty() || signature.generics.where_clause.is_some() {
        return Err(LowerError::new(
            signature.generics.span(),
            "generic functions cannot cross the binding boundary; export a \
             monomorphic wrapper",
        ));
    }
    if let Some(variadic) = &signature.variadic {
        return Err(LowerError::new(
            variadic.span(),
            "variadic functions do not cross the binding boundary",
        ));
    }

    let meta = attrs::UnibindMeta::from_attrs(&func.attrs)?;
    meta.reject_default("a function")?;
    meta.reject_py_base("a function")?;

    let mut args = Vec::new();
    for input in &signature.inputs {
        let arg = match input {
            syn::FnArg::Receiver(receiver) => {
                return Err(LowerError::new(
                    receiver.span(),
                    "methods belong to #[unibind::object], which lands in \
                     phase 2 (issue #1992)",
                ));
            }
            syn::FnArg::Typed(arg) => arg,
        };
        args.push(lower_arg(arg, declared)?);
    }
    check_default_order(signature, &args)?;

    let returned = lower_return(&signature.output, declared)?;
    Ok(ir::Function {
        name: signature.ident.to_string(),
        names: meta.names(),
        docs: attrs::doc_lines(&func.attrs),
        asyncness: ir::Asyncness::Sync,
        args,
        ret: returned.ty,
        throws: returned.throws,
    })
}

fn lower_arg(arg: &syn::PatType, declared: &Declared) -> Result<ir::Arg> {
    let syn::Pat::Ident(pattern) = &*arg.pat else {
        return Err(LowerError::new(
            arg.pat.span(),
            "exported function arguments need plain identifier names",
        ));
    };
    let meta = attrs::UnibindMeta::from_attrs(&arg.attrs)?;
    meta.reject_py_base("an argument")?;
    Ok(ir::Arg {
        name: pattern.ident.to_string(),
        names: meta.names(),
        ty: lower_type(&arg.ty, declared, Position::Arg)?,
        default: meta.default,
    })
}

/// Python only accepts defaulted parameters after other defaulted ones, so
/// enforce the same shape here: once an argument has a default (explicit, or
/// the implicit `None` of an `Option`), every later argument needs one.
fn check_default_order(signature: &syn::Signature, args: &[ir::Arg]) -> Result<()> {
    let mut defaults_started = false;
    for arg in args {
        let has_default = arg.default.is_some() || matches!(arg.ty, ir::Type::Option(_));
        if defaults_started && !has_default {
            return Err(LowerError::new(
                signature.span(),
                format!(
                    "argument `{}` needs a default: it follows a defaulted argument",
                    arg.name
                ),
            ));
        }
        defaults_started = defaults_started || has_default;
    }
    Ok(())
}

/// A lowered return position: the success type and the thrown error, if any.
struct Returned {
    ty: Option<ir::Type>,
    throws: Option<String>,
}

fn lower_return(output: &syn::ReturnType, declared: &Declared) -> Result<Returned> {
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
    if let syn::Type::Path(path) = &**ty
        && let Some(segment) = path.path.segments.last()
        && segment.ident == "Result"
    {
        return lower_result(segment, declared);
    }
    Ok(Returned {
        ty: Some(lower_type(ty, declared, Position::Owned)?),
        throws: None,
    })
}

fn lower_result(segment: &syn::PathSegment, declared: &Declared) -> Result<Returned> {
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
    let ty = if is_unit(ok) {
        None
    } else {
        Some(lower_type(ok, declared, Position::Owned)?)
    };
    Ok(Returned {
        ty,
        throws: Some(error_ident.to_string()),
    })
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
