//! Generate the `#[napi(object)]` twin of a record whose JavaScript shape
//! differs from the user's own struct.
//!
//! napi derives a record's conversions from the field types it can see, and
//! those are the user's own. Two field types need a different declaration
//! than the user wrote:
//!
//! - A `u64` field has no napi conversion at all and an `i64` field would
//!   cross through an explicit, checked adaptation, so both are declared
//!   in glue-owned shapes (`f64` for wide integers, `Buffer` for bytes), with
//!   the narrowing (and its refusal) on the way back in.
//! - A `Vec<u8>` field would cross as `Array<number>`: one JavaScript number
//!   object per byte, and no `Buffer` for the caller to hand to anything
//!   that takes bytes. It is declared `Buffer`, which is a legal
//!   `#[napi(object)]` field type in both directions.
//!
//! The mirror keeps the same JavaScript shape and the same key names.
//! Records whose fields all cross unchanged keep `#[napi(object)]` on the
//! user's struct exactly as before, so the mirror only exists where it
//! changes something.

use proc_macro2::TokenStream;
use quote::quote;
use unibind_core::ir;
use unibind_core::render::{RenderError, name_ident};

use crate::convert;
use crate::ty::{self, Level, TyCtx};

/// The records that must cross through a mirror: those carrying a 64-bit
/// integer or a field of bytes directly, plus those reaching one through
/// another record. The fixpoint runs to a stop because each pass can only
/// grow the set, which is bounded by the record count.
pub fn mirrored_records(records: &[ir::Record]) -> Vec<String> {
    let mut mirrored: Vec<String> = Vec::new();
    loop {
        let mut grew = false;
        for record in records {
            if mirrored.contains(&record.name) {
                continue;
            }
            if record
                .fields
                .iter()
                .any(|field| convert::adapts(&field.ty, &mirrored, Level::Field))
            {
                mirrored.push(record.name.clone());
                grew = true;
            }
        }
        if !grew {
            return mirrored;
        }
    }
}

/// The mirror struct for one record plus the two conversions that move a
/// value across it.
pub fn render_mirror(record: &ir::Record, ctx: &TyCtx<'_>) -> Result<TokenStream, RenderError> {
    let user = ctx.user;
    let name = name_ident(&record.name)?;
    let mirror = convert::mirror_ident(&record.name);
    let outer = record.names.ts.as_ref().map_or_else(
        || quote!(#[::napi_derive::napi(object)]),
        |js_name| quote!(#[::napi_derive::napi(object, js_name = #js_name)]),
    );

    let mut fields = Vec::new();
    let mut from = Vec::new();
    let mut into = Vec::new();
    for field in &record.fields {
        let ident = name_ident(&field.name)?;
        let declared = ty::decl(&field.ty, ctx, Level::Field)?;
        let rename = field
            .names
            .ts
            .as_ref()
            .map(|js_name| quote!(#[napi(js_name = #js_name)]));
        fields.push(quote! {
            #rename
            pub #ident: #declared,
        });

        // Bytes are asked first and separately: theirs is the one field
        // adaptation that cannot fail, so it carries no `?`.
        let read = quote!(value.#ident);
        let widened = convert::bytes_field_outward(&field.ty, &read)
            .or_else(|| convert::outward(&field.ty, ctx, &read))
            .map_or_else(
                || quote!(#ident: #read,),
                |converted| quote!(#ident: #converted,),
            );
        from.push(widened);

        let take = quote!(self.#ident);
        let narrowed = if let Some(converted) = convert::bytes_field_inward(&field.ty, &take) {
            quote!(#ident: #converted,)
        } else {
            convert::inward(&field.ty, ctx, &take).map_or_else(
                || quote!(#ident: #take,),
                |converted| quote!(#ident: #converted?,),
            )
        };
        into.push(narrowed);
    }

    Ok(quote! {
        #outer
        pub struct #mirror {
            #(#fields)*
        }

        // A record declared but never mentioned in a signature leaves both
        // conversions uncalled; that is the user's business, not a glue
        // defect to report as dead code.
        #[allow(dead_code)]
        impl #mirror {
            /// The record as JavaScript sees it.
            fn __unibind_from(value: #user::#name) -> Self {
                Self { #(#from)* }
            }

            /// The record as the user's code takes it; a `number` outside a
            /// field's declared width is refused here rather than truncated.
            fn __unibind_into(self) -> ::napi::Result<#user::#name> {
                ::std::result::Result::Ok(#user::#name { #(#into)* })
            }
        }
    })
}
