//! Build one envelope value from a success expression.
//!
//! The sync exports evaluate the user call directly and the async exports
//! evaluate the `TaskOutcome::Completed` payload, so the construction here
//! takes an arbitrary value expression and both paths share the `throws`
//! matching and the success-value encoding.

use proc_macro2::{Ident, Literal, TokenStream};
use quote::quote;
use unibind_core::ir;

use crate::model::Model;
use crate::rust_glue::encode;
use crate::{names, RenderError};

/// Everything envelope construction needs besides the value expression.
pub(crate) struct EnvelopeParts<'a> {
    pub interface: &'a ir::Interface,
    pub model: &'a Model<'a>,
    pub user: &'a Ident,
    pub function: &'a ir::Function,
    /// The boundary success type; `None` for unit. Constructors pass the
    /// object's own type here even though their IR `ret` is `None`.
    pub ret: Option<&'a ir::Type>,
    pub envelope: &'a Ident,
}

/// The `value: ...` field tokens zeroing the payload on the error, panic,
/// and cancel arms; `None` when the envelope has no payload.
pub(crate) fn zero_value(parts: &EnvelopeParts<'_>) -> Option<TokenStream> {
    parts
        .ret
        .map(|_| quote!(value: unsafe { ::core::mem::zeroed() },))
}

/// The expression building the envelope from `value_expr`, which evaluates
/// to the function's raw return (a `Result` when it throws).
pub(crate) fn envelope_expr(
    parts: &EnvelopeParts<'_>,
    value_expr: &TokenStream,
) -> Result<TokenStream, RenderError> {
    let ok_value = match parts.ret {
        Some(ty) => {
            let encoded = ret_encode(parts.model, ty, &quote!(value))?;
            Some(quote!(value: #encoded,))
        }
        None => None,
    };
    match &parts.function.throws {
        Some(throws) => throws_expr(parts, throws, value_expr, ok_value.as_ref()),
        None => Ok(plain_expr(parts, value_expr, ok_value.as_ref())),
    }
}

/// Encode one boundary success value: streams and objects leave as raw
/// handles, everything else through the mirror encoders. Streams hand the
/// runtime a boxed shared stream; objects leak an `Arc` whose count the
/// object's `__free` export releases.
fn ret_encode(
    model: &Model<'_>,
    ty: &ir::Type,
    access: &TokenStream,
) -> Result<TokenStream, RenderError> {
    match ty {
        ir::Type::Stream(_) => Ok(quote! {
            ::unibind_runtime::jvm::stream_into_raw(#access)
        }),
        ir::Type::Named(name) if model.is_object(name) => Ok(quote! {
            ::std::sync::Arc::into_raw(::std::sync::Arc::new(#access))
                .cast_mut()
                .cast::<::core::ffi::c_void>()
        }),
        _ => encode::expr(model, ty, access),
    }
}

fn plain_expr(
    parts: &EnvelopeParts<'_>,
    value_expr: &TokenStream,
    ok_value: Option<&TokenStream>,
) -> TokenStream {
    let envelope = parts.envelope;
    if parts.ret.is_some() {
        quote! {
            {
                let value = #value_expr;
                #envelope {
                    code: 0,
                    err_msg: null_string(),
                    #ok_value
                }
            }
        }
    } else {
        // `let () = ...` both consumes the unit value without tripping the
        // `path_statements` lint (the async path's value expression is a
        // bare binding) and proves the expression really is unit.
        quote! {
            {
                let () = #value_expr;
                #envelope {
                    code: 0,
                    err_msg: null_string(),
                }
            }
        }
    }
}

fn throws_expr(
    parts: &EnvelopeParts<'_>,
    throws: &str,
    value_expr: &TokenStream,
    ok_value: Option<&TokenStream>,
) -> Result<TokenStream, RenderError> {
    let error = parts
        .interface
        .errors
        .iter()
        .find(|error| error.name == *throws)
        .expect("throws names are validated when the model is built");
    let envelope = parts.envelope;
    let user = parts.user;
    let error_ident = names::rust_ident(&error.name)?;
    let mut arms = Vec::new();
    for (index, variant) in error.variants.iter().enumerate() {
        let variant_ident = names::rust_ident(&variant.name)?;
        let code = Literal::usize_unsuffixed(index + 1);
        arms.push(quote! {
            super::#user::#error_ident::#variant_ident { .. } => #code,
        });
    }
    let ok_pattern = if parts.ret.is_some() {
        quote!(value)
    } else {
        quote!(())
    };
    let zero_value = zero_value(parts);
    Ok(quote! {
        match #value_expr {
            ::std::result::Result::Ok(#ok_pattern) => #envelope {
                code: 0,
                err_msg: null_string(),
                #ok_value
            },
            ::std::result::Result::Err(error) => #envelope {
                code: match &error {
                    #(#arms)*
                },
                err_msg: string_value(::std::string::ToString::to_string(&error)),
                #zero_value
            },
        }
    })
}
