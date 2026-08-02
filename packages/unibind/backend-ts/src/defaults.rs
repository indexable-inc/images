//! Substitute declared defaults for arguments JavaScript omitted.

use proc_macro2::{Ident, TokenStream};
use quote::quote;
use unibind_core::ir;
use unibind_core::render::RenderError;

use crate::convert;
use crate::function::Binding;
use crate::ty::{self, TyCtx};

/// How an omittable argument reaches the call. A type that crosses
/// unchanged substitutes its default in place; one whose boundary shape
/// differs is adapted first, in a prologue statement that can refuse the
/// value, and only then falls back to the default.
///
/// # Errors
///
/// Fails for a default paired with a type that cannot carry one.
pub fn defaulted(
    arg: &ir::Arg,
    default: &ir::Literal,
    ident: &Ident,
    function: &ir::Function,
    ctx: &TyCtx<'_>,
) -> Result<Binding, RenderError> {
    let Some(converted) = convert::inward(&arg.ty, ctx, &quote!(#ident)) else {
        return Ok(Binding {
            prologue: None,
            expr: substituted(arg, default, ident, function)?,
        });
    };
    // The adapted set is the 64-bit integers plus the records and
    // containers holding them, and only the integers can carry a default.
    let (ir::Type::Int(_), ir::Literal::Int(value)) = (&arg.ty, default) else {
        return Err(unsupported_default(arg, function));
    };
    let value = proc_macro2::Literal::i64_unsuffixed(*value);
    // The `Some` arm rebinds the same name, so `converted` (written against
    // `ident`) reads the unwrapped value.
    Ok(Binding {
        prologue: Some(quote! {
            let #ident = match #ident {
                ::std::option::Option::Some(#ident) => #converted?,
                ::std::option::Option::None => #value,
            };
        }),
        expr: quote!(#ident),
    })
}

/// The [`defaulted`] counterpart for an `Option` argument: JavaScript's
/// omission is already the argument's own shape, so the adaptation runs
/// first and the default fills the resulting `None` afterwards.
///
/// # Errors
///
/// Fails for a default the argument's element type cannot carry.
pub fn optional_defaulted(
    arg: &ir::Arg,
    default: &ir::Literal,
    ident: &Ident,
    function: &ir::Function,
    ctx: &TyCtx<'_>,
) -> Result<Binding, RenderError> {
    let ir::Type::Option(inner) = &arg.ty else {
        return Err(unsupported_default(arg, function));
    };
    let Some(converted) = convert::inward(&arg.ty, ctx, &quote!(#ident)) else {
        return Ok(Binding {
            prologue: None,
            expr: option_substituted(arg, default, ident, function)?,
        });
    };
    if matches!(default, ir::Literal::None) {
        return Ok(Binding {
            prologue: Some(quote!(let #ident = #converted?;)),
            expr: quote!(#ident),
        });
    }
    let (ir::Type::Int(_), ir::Literal::Int(value)) = (&**inner, default) else {
        return Err(unsupported_default(arg, function));
    };
    let value = proc_macro2::Literal::i64_unsuffixed(*value);
    Ok(Binding {
        prologue: Some(quote! {
            let #ident = #converted?.or(::std::option::Option::Some(#value));
        }),
        expr: quote!(#ident),
    })
}

/// The call-site expression for an argument JavaScript may omit: `None`
/// falls back to the declared default.
fn substituted(
    arg: &ir::Arg,
    default: &ir::Literal,
    ident: &Ident,
    function: &ir::Function,
) -> Result<TokenStream, RenderError> {
    Ok(match (&arg.ty, default) {
        (ir::Type::Bool, ir::Literal::Bool(value)) => quote!(#ident.unwrap_or(#value)),
        (ir::Type::Int(_), ir::Literal::Int(value)) => {
            let value = proc_macro2::Literal::i64_unsuffixed(*value);
            quote!(#ident.unwrap_or(#value))
        }
        (ir::Type::Float(_), ir::Literal::Float(value)) => {
            let value = proc_macro2::Literal::f64_unsuffixed(*value);
            quote!(#ident.unwrap_or(#value))
        }
        (ir::Type::Float(_), ir::Literal::Int(value)) => {
            let value = float_literal(*value)?;
            quote!(#ident.unwrap_or(#value))
        }
        (ir::Type::String { owned: false }, ir::Literal::Str(value)) => {
            quote!(#ident.as_deref().unwrap_or(#value))
        }
        (ir::Type::String { owned: true }, ir::Literal::Str(value)) => {
            quote!(#ident.unwrap_or_else(|| ::std::string::String::from(#value)))
        }
        (ir::Type::Path { owned: false }, ir::Literal::Str(value)) => {
            quote!(#ident.as_deref().unwrap_or_else(|| ::std::path::Path::new(#value)))
        }
        (ir::Type::Path { owned: true }, ir::Literal::Str(value)) => {
            quote!(#ident.unwrap_or_else(|| ::std::path::PathBuf::from(#value)))
        }
        _ => return Err(unsupported_default(arg, function)),
    })
}

fn unsupported_default(arg: &ir::Arg, function: &ir::Function) -> RenderError {
    RenderError::new(format!(
        "argument `{}` of `{}` pairs a default with a type the ts backend \
         cannot substitute; keep defaults on bool, numbers, strings, and \
         paths (issue #1993)",
        arg.name, function.name,
    ))
}

/// The substitution for an omitted `Option` argument carrying an explicit
/// default: `None` from JavaScript becomes `Some(default)`, except for the
/// `None` default, which the argument shape already expresses.
fn option_substituted(
    arg: &ir::Arg,
    default: &ir::Literal,
    ident: &Ident,
    function: &ir::Function,
) -> Result<TokenStream, RenderError> {
    let ir::Type::Option(inner) = &arg.ty else {
        return Err(unsupported_default(arg, function));
    };
    let some = |value: TokenStream| quote!(::std::option::Option::Some(#value));
    Ok(match (&**inner, default) {
        (_, ir::Literal::None) => ty::pass(&arg.ty, &quote!(#ident)),
        (ir::Type::Bool, ir::Literal::Bool(value)) => {
            let value = some(quote!(#value));
            quote!(#ident.or(#value))
        }
        (ir::Type::Int(_), ir::Literal::Int(value)) => {
            let value = proc_macro2::Literal::i64_unsuffixed(*value);
            let value = some(quote!(#value));
            quote!(#ident.or(#value))
        }
        (ir::Type::Float(_), ir::Literal::Float(value)) => {
            let value = proc_macro2::Literal::f64_unsuffixed(*value);
            let value = some(quote!(#value));
            quote!(#ident.or(#value))
        }
        (ir::Type::Float(_), ir::Literal::Int(value)) => {
            let value = float_literal(*value)?;
            let value = some(quote!(#value));
            quote!(#ident.or(#value))
        }
        (ir::Type::String { owned: false }, ir::Literal::Str(value)) => {
            let value = some(quote!(#value));
            quote!(#ident.as_deref().or(#value))
        }
        (ir::Type::String { owned: true }, ir::Literal::Str(value)) => {
            let value = some(quote!(::std::string::String::from(#value)));
            quote!(#ident.or_else(|| #value))
        }
        (ir::Type::Path { owned: false }, ir::Literal::Str(value)) => {
            let value = some(quote!(::std::path::Path::new(#value)));
            quote!(#ident.as_deref().or_else(|| #value))
        }
        (ir::Type::Path { owned: true }, ir::Literal::Str(value)) => {
            let value = some(quote!(::std::path::PathBuf::from(#value)));
            quote!(#ident.or_else(|| #value))
        }
        _ => return Err(unsupported_default(arg, function)),
    })
}

/// An `i64` default rendered as an exact `f64` literal (`10` -> `10.0`).
/// Formatting instead of casting keeps the token faithful to the source
/// digits for every i64.
fn float_literal(value: i64) -> Result<proc_macro2::Literal, RenderError> {
    format!("{value}.0")
        .parse()
        .map_err(|_| RenderError::new(format!("`{value}` is not renderable as a float default")))
}
