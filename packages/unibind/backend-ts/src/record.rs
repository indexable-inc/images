//! Attach `#[napi(object)]` to record structs.
//!
//! A record crosses as a plain JavaScript object: napi generates both
//! `FromNapiValue` and `ToNapiValue` from the pub fields, so values flow in
//! both directions with no constructor to register. Unrenamed fields follow
//! napi's own camelCase convention; `ts(name = ...)` renames pin an exact
//! key.

use syn::parse_quote;
use unibind_core::ir;

use crate::{RenderedRecord, ty};

/// The attributes the exported struct gains: `#[napi(object)]` on the item
/// and a `js_name` per renamed field. The bare `napi` field attributes are
/// consumed (and stripped) by the outer `napi(object)` expansion.
pub fn record_attrs(record: &ir::Record) -> RenderedRecord {
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
pub fn check_record(record: &ir::Record) -> Result<(), crate::RenderError> {
    for field in &record.fields {
        ty::check(
            &field.ty,
            &format!("field `{}` of record `{}`", field.name, record.name),
        )?;
    }
    Ok(())
}
