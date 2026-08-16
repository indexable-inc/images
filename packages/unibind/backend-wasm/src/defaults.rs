//! Substitute declared defaults for arguments JavaScript omitted.
//!
//! Which of the two shapes applies is whether the argument's *value* differs
//! between the two sides. One that crosses unchanged substitutes in place, in
//! the boundary spelling, and [`ty::pass`] then rebuilds what the user's
//! function takes -- so a path's default is a JavaScript string like every
//! other path, and the reborrowing is written once rather than once per literal
//! kind. One whose value is adapted gets a prologue statement that can refuse,
//! and the default fills the omission there.

use proc_macro2::{Ident, Literal, TokenStream};
use quote::quote;
use unibind_core::ir;
use unibind_core::render::RenderError;

use crate::convert;
use crate::function::{Binding, refused};
use crate::ty::{self, TyCtx};

/// How an omittable argument reaches the call.
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
    let Some(converted) = convert::inward(&arg.ty, ctx, &quote!(#ident))? else {
        let value = substituted(arg, default, ident, function)?;
        return Ok(Binding {
            prologue: None,
            expr: ty::pass(&arg.ty, &value),
        });
    };
    // The adapted set is the 64-bit integers, the unit enums, and the records
    // and containers holding them; of those only an integer can carry a
    // literal default.
    let (ir::Type::Int(_), ir::Literal::Int(value)) = (&arg.ty, default) else {
        return Err(unsupported_default(arg, function));
    };
    let value = Literal::i64_unsuffixed(*value);
    let converted = refused(&converted);
    // The `Some` arm rebinds the same name, so `converted` (written against
    // `ident`) reads the unwrapped value.
    Ok(Binding {
        prologue: Some(quote! {
            let #ident = match #ident {
                ::std::option::Option::Some(#ident) => #converted,
                ::std::option::Option::None => #value,
            };
        }),
        expr: quote!(#ident),
    })
}

/// The [`defaulted`] counterpart for an `Option` argument: JavaScript's
/// omission is already the argument's own shape, so the adaptation runs first
/// and the default fills the resulting `None` afterwards.
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
    let Some(converted) = convert::inward(&arg.ty, ctx, &quote!(#ident))? else {
        let value = option_substituted(arg, default, ident, function)?;
        return Ok(Binding {
            prologue: None,
            expr: ty::pass(&arg.ty, &value),
        });
    };
    let converted = refused(&converted);
    if matches!(default, ir::Literal::None) {
        return Ok(Binding {
            prologue: Some(quote!(let #ident = #converted;)),
            expr: quote!(#ident),
        });
    }
    let (ir::Type::Int(_), ir::Literal::Int(value)) = (&**inner, default) else {
        return Err(unsupported_default(arg, function));
    };
    let value = Literal::i64_unsuffixed(*value);
    Ok(Binding {
        prologue: Some(quote! {
            let #ident = #converted.or(::std::option::Option::Some(#value));
        }),
        expr: quote!(#ident),
    })
}

/// The value for an argument JavaScript may omit, in the shape the wrapper
/// declared it: `None` falls back to the declared default. A string and a path
/// share an arm, because a path crosses as a string.
fn substituted(
    arg: &ir::Arg,
    default: &ir::Literal,
    ident: &Ident,
    function: &ir::Function,
) -> Result<TokenStream, RenderError> {
    Ok(match (&arg.ty, default) {
        (ir::Type::Bool, ir::Literal::Bool(value)) => quote!(#ident.unwrap_or(#value)),
        (ir::Type::Int(_), ir::Literal::Int(value)) => {
            let value = Literal::i64_unsuffixed(*value);
            quote!(#ident.unwrap_or(#value))
        }
        (ir::Type::Float(_), ir::Literal::Float(value)) => {
            let value = Literal::f64_unsuffixed(*value);
            quote!(#ident.unwrap_or(#value))
        }
        (ir::Type::Float(_), ir::Literal::Int(value)) => {
            let value = float_literal(*value)?;
            quote!(#ident.unwrap_or(#value))
        }
        (ir::Type::String { .. } | ir::Type::Path { .. }, ir::Literal::Str(value)) => {
            quote!(#ident.unwrap_or_else(|| ::std::string::String::from(#value)))
        }
        _ => return Err(unsupported_default(arg, function)),
    })
}

/// The [`substituted`] counterpart for an omitted `Option` argument carrying an
/// explicit default: `None` from JavaScript becomes `Some(default)`, except for
/// the `None` default, which the argument shape already expresses.
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
        (_, ir::Literal::None) => quote!(#ident),
        (ir::Type::Bool, ir::Literal::Bool(value)) => {
            let value = some(quote!(#value));
            quote!(#ident.or(#value))
        }
        (ir::Type::Int(_), ir::Literal::Int(value)) => {
            let value = Literal::i64_unsuffixed(*value);
            let value = some(quote!(#value));
            quote!(#ident.or(#value))
        }
        (ir::Type::Float(_), ir::Literal::Float(value)) => {
            let value = Literal::f64_unsuffixed(*value);
            let value = some(quote!(#value));
            quote!(#ident.or(#value))
        }
        (ir::Type::Float(_), ir::Literal::Int(value)) => {
            let value = float_literal(*value)?;
            let value = some(quote!(#value));
            quote!(#ident.or(#value))
        }
        (ir::Type::String { .. } | ir::Type::Path { .. }, ir::Literal::Str(value)) => {
            let value = some(quote!(::std::string::String::from(#value)));
            quote!(#ident.or_else(|| #value))
        }
        _ => return Err(unsupported_default(arg, function)),
    })
}

fn unsupported_default(arg: &ir::Arg, function: &ir::Function) -> RenderError {
    RenderError::new(format!(
        "argument `{}` of `{}` pairs a default with a type the wasm backend \
         cannot substitute; keep defaults on bool, numbers, strings, and paths \
         (issue #1993)",
        arg.name, function.name,
    ))
}

/// An `i64` default rendered as an exact `f64` literal (`10` -> `10.0`).
/// Formatting instead of casting keeps the token faithful to the source digits
/// for every `i64`.
fn float_literal(value: i64) -> Result<Literal, RenderError> {
    format!("{value}.0")
        .parse()
        .map_err(|_| RenderError::new(format!("`{value}` is not renderable as a float default")))
}
