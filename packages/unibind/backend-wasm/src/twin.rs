//! Generate the serde twin of a record: the struct that actually crosses.
//!
//! `wasm-bindgen` has no attribute that makes a plain struct cross by value,
//! so a record reaches JavaScript through `serde`, and the twin is where every
//! boundary spelling lives without the user's own struct mentioning `serde` or
//! `wasm-bindgen`:
//!
//! - The keys are the JavaScript names ([`crate::names`]), pinned with
//!   `serde(rename = ...)` rather than a container-wide `rename_all`, so a
//!   field's `ts(name = ...)` and the default `camelCase` are one mechanism.
//! - A 64-bit field is declared `f64`, with the checked narrowing (and its
//!   refusal) on the way back in.
//! - A unit-enum field is declared `String`, its wire spelling.
//! - A nested record's field is the nested twin, so one `serde` move carries
//!   the whole tree.
//!
//! Unconditional, unlike the ts backend's mirror structs: there is no "the
//! spellings agree, so skip it" case, because no record crosses without a
//! twin at all. That also means the fixpoint the ts backend needs (a record
//! reaching a mirrored record through a field) has nothing to compute.

use proc_macro2::TokenStream;
use quote::quote;
use unibind_core::ir;
use unibind_core::render::{RenderError, name_ident};

use crate::ty::{self, Level, TyCtx};
use crate::{convert, names};

/// One record's twin plus the two conversions that move a value across it.
pub fn render_twin(record: &ir::Record, ctx: &TyCtx<'_>) -> Result<TokenStream, RenderError> {
    let user = ctx.user;
    let name = name_ident(&record.name)?;
    let twin = convert::twin_ident(&record.name);

    let mut fields = Vec::new();
    let mut from = Vec::new();
    let mut into = Vec::new();
    for field in &record.fields {
        let ident = name_ident(&field.name)?;
        let declared = ty::decl(&field.ty, ctx, Level::Inner)?;
        let key = names::js_member(&field.names, &field.name);
        // An optional field is one a JavaScript caller omits rather than sets
        // to `undefined`, which is what napi's `Option` fields already accept;
        // without this serde refuses the object for a missing key.
        let absent = matches!(field.ty, ir::Type::Option(_)).then(|| quote!(#[serde(default)]));
        fields.push(quote! {
            #[serde(rename = #key)]
            #absent
            pub #ident: #declared,
        });

        let read = quote!(value.#ident);
        from.push(
            convert::outward(&field.ty, ctx, &read).map_or_else(
                || quote!(#ident: #read,),
                |converted| quote!(#ident: #converted,),
            ),
        );

        let take = quote!(self.#ident);
        into.push(
            convert::inward_serde(&field.ty, ctx, &take).map_or_else(
                || quote!(#ident: #take,),
                |converted| quote!(#ident: #converted?,),
            ),
        );
    }

    Ok(quote! {
        #[derive(::serde::Serialize, ::serde::Deserialize)]
        pub struct #twin {
            #(#fields)*
        }

        // A record declared but never mentioned in a signature leaves both
        // conversions uncalled; that is the user's business, not a glue defect
        // to report as dead code.
        #[allow(dead_code)]
        impl #twin {
            /// The record as JavaScript sees it.
            fn __unibind_from(value: #user::#name) -> Self {
                Self { #(#from)* }
            }

            /// The record as the user's code takes it; a `number` outside a
            /// field's declared width, or a word outside a field enum's set, is
            /// refused here rather than coerced.
            fn __unibind_into(
                self,
            ) -> ::std::result::Result<#user::#name, ::std::string::String> {
                ::std::result::Result::Ok(#user::#name { #(#into)* })
            }
        }
    })
}
