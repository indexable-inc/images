//! Convert error enums into `JsValue` rejections with machine-decodable
//! messages.
//!
//! JavaScript has no exception hierarchy to register from Rust, so the variant
//! identity crosses inside the thrown error's message:
//! `__unibind__:err:<ErrorName>:<VariantName>:<Display message>`. The two names
//! are Rust identifiers (never containing `:`), and the message is the final
//! field, so splitting on the first four colons is lossless. The generated
//! JavaScript decodes the prefix into real `Error` subclasses named by the IR's
//! renames; the glue only guarantees the channel.
//!
//! Identical to the ts backend's channel, down to the prefix: one wire
//! vocabulary means one decoder, whichever artifact loaded.

use proc_macro2::{Ident, TokenStream};
use quote::{format_ident, quote};
use unibind_core::ir;
use unibind_core::render::{RenderError, name_ident};

use crate::function::doc_attrs;

/// The message prefix of every error the glue raises on purpose.
pub const REASON_PREFIX: &str = "__unibind__";

/// The identifier of the generated conversion for error enum `name`.
///
/// A named function rather than a `From` impl: `JsValue` is foreign and an
/// inherent conversion is one item the reader can find, while a blanket
/// `From<E> for JsValue` in the user's own crate would sit next to
/// `wasm-bindgen`'s own impls for anyone else to trip over.
pub fn conversion_ident(name: &str) -> Ident {
    format_ident!("__unibind_wasm_err_{}", name)
}

pub fn render_error(error: &ir::ErrorType, user: &Ident) -> Result<TokenStream, RenderError> {
    let rust_name = name_ident(&error.name)?;
    let conversion = conversion_ident(&error.name);
    let mut arms = Vec::new();
    for variant in &error.variants {
        let variant_ident = name_ident(&variant.name)?;
        let reason = format!("{REASON_PREFIX}:err:{}:{}:", error.name, variant.name);
        arms.push(quote! {
            #user::#rust_name::#variant_ident { .. } => {
                __unibind_wasm_error(::std::format!("{}{}", #reason, message))
            }
        });
    }

    let docs = doc_attrs(&[format!(
        " Map `{}` onto a decodable rejection message, text from `Display`.",
        error.name
    )]);
    Ok(quote! {
        #docs
        #[allow(dead_code)]
        fn #conversion(error: #user::#rust_name) -> ::wasm_bindgen::JsValue {
            let message = ::std::string::ToString::to_string(&error);
            match error {
                #(#arms)*
            }
        }
    })
}
