//! Convert error enums into `napi::Error` values with machine-decodable
//! reasons.
//!
//! JavaScript has no exception hierarchy to register from Rust, so the
//! variant identity crosses inside the rejection reason:
//! `__unibind__:err:<ErrorName>:<VariantName>:<Display message>`. The two
//! names are Rust identifiers (never containing `:`), and the message is
//! the final field, so splitting on the first four colons is lossless.
//! Stage 2's generated `index.js` decodes the prefix into real `Error`
//! subclasses named by the IR's ts renames; the glue only guarantees the
//! channel.

use proc_macro2::{Ident, TokenStream};
use quote::quote;
use unibind_core::ir;

use crate::function::doc_attrs;
use crate::ty::name_ident;
use crate::RenderError;

/// The reason prefix of every error the glue raises on purpose.
pub const REASON_PREFIX: &str = "__unibind__";

pub fn render_error(error: &ir::ErrorType, user: &Ident) -> Result<TokenStream, RenderError> {
    let rust_name = name_ident(&error.name)?;
    let mut arms = Vec::new();
    for variant in &error.variants {
        let variant_ident = name_ident(&variant.name)?;
        let reason = format!("{REASON_PREFIX}:err:{}:{}:", error.name, variant.name);
        arms.push(quote! {
            super::#user::#rust_name::#variant_ident { .. } => {
                ::napi::Error::from_reason(::std::format!("{}{}", #reason, message))
            }
        });
    }

    let from_docs = doc_attrs(&[format!(
        "Map `{}` onto a decodable napi rejection reason, message from `Display`.",
        error.name
    )]);
    Ok(quote! {
        #from_docs
        impl ::std::convert::From<super::#user::#rust_name> for ::napi::Error {
            fn from(error: super::#user::#rust_name) -> Self {
                let message = ::std::string::ToString::to_string(&error);
                match error {
                    #(#arms)*
                }
            }
        }
    })
}
