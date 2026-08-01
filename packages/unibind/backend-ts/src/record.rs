//! Attach `#[napi(object)]` to record structs.
//!
//! A record crosses as a plain JavaScript object: napi generates both
//! `FromNapiValue` and `ToNapiValue` from the pub fields, so values flow in
//! both directions with no constructor to register. Unrenamed fields follow
//! napi's own camelCase convention; `ts(name = ...)` renames pin an exact
//! key.
//!
//! A record carrying a 64-bit integer is the exception: napi would read the
//! user's own field type, which cannot carry one faithfully, so the glue
//! renders a mirror struct instead (see [`crate::mirror`]) and the user's
//! struct gains no attributes at all.

use syn::parse_quote;
use unibind_core::ir;
use unibind_core::render::{RenderError, RenderedRecord};

use crate::ty;

/// The attributes the exported struct gains: `#[napi(object)]` on the item
/// and a `js_name` per renamed field. The bare `napi` field attributes are
/// consumed (and stripped) by the outer `napi(object)` expansion. A
/// mirrored record gains none: its mirror carries them.
pub fn record_attrs(record: &ir::Record, mirrored: &[String]) -> RenderedRecord {
    if mirrored.iter().any(|name| *name == record.name) {
        return RenderedRecord {
            outer: Vec::new(),
            fields: record.fields.iter().map(|_| Vec::new()).collect(),
        };
    }
    let outer: syn::Attribute = record.names.ts.as_ref().map_or_else(
        || parse_quote!(#[::napi_derive::napi(object)]),
        |name| parse_quote!(#[::napi_derive::napi(object, js_name = #name)]),
    );
    let fields = record
        .fields
        .iter()
        .map(|field| {
            field.names.ts.as_ref().map_or_else(Vec::new, |name| {
                let attr: syn::Attribute = parse_quote!(#[napi(js_name = #name)]);
                vec![attr]
            })
        })
        .collect();
    RenderedRecord {
        outer: vec![outer],
        fields,
    }
}

/// Field types must be napi-representable in both directions; check them
/// before the struct picks up attributes that would miscompile.
pub fn check_record(record: &ir::Record) -> Result<(), RenderError> {
    for field in &record.fields {
        ty::check(
            &field.ty,
            &format!("field `{}` of record `{}`", field.name, record.name),
        )?;
    }
    Ok(())
}
