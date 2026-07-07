//! Render error enums as transparent bridge enums whose variants carry the
//! Rust `Display` text, so Swift throws a native enum with the message
//! attached.

use proc_macro2::{Ident, Span, TokenStream};
use quote::quote;
use unibind_core::ir;

use crate::names;
use crate::RenderError;

/// One rendered error: the transparent enum inside the bridge module, and
/// the `From` impl in the glue module that maps the user's enum onto it.
pub struct RenderedError {
    pub bridge_enum: TokenStream,
    pub items: TokenStream,
}

/// The bridge-side enum identifier (`__UnibindSampleError`).
pub fn bridge_ident(error: &ir::ErrorType) -> Ident {
    Ident::new(&format!("__Unibind{}", error.name), Span::call_site())
}

pub fn render_error(
    error: &ir::ErrorType,
    ffi_mod: &Ident,
    user: &Ident,
) -> Result<RenderedError, RenderError> {
    let rust_name = names::name_ident(&error.name)?;
    let bridge_name = bridge_ident(error);

    let mut variants = Vec::new();
    let mut arms = Vec::new();
    for variant in &error.variants {
        let ident = names::name_ident(&variant.name)?;
        variants.push(quote!(#ident(String)));
        arms.push(quote! {
            super::#user::#rust_name::#ident { .. } => Self::#ident(message),
        });
    }

    let bridge_enum = quote! {
        enum #bridge_name {
            #(#variants),*
        }
    };
    let items = quote! {
        impl ::std::convert::From<super::#user::#rust_name> for #ffi_mod::#bridge_name {
            fn from(error: super::#user::#rust_name) -> Self {
                let message = ::std::string::ToString::to_string(&error);
                match error {
                    #(#arms)*
                }
            }
        }
    };
    Ok(RenderedError {
        bridge_enum,
        items,
    })
}
