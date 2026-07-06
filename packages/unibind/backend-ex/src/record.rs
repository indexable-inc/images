//! Derive `NifStruct` onto record structs.

use syn::parse_quote;
use unibind_core::ir;

use crate::{names, ty, RenderError, RenderedRecord};

/// The attributes the exported struct gains: a `NifStruct` derive mapping
/// it onto `%<Ns>.<Record>{}` (rustler prepends the `Elixir.` itself).
/// Fields gain nothing; the derive reads them in place.
pub fn record_attrs(record: &ir::Record, ns: &str) -> Result<RenderedRecord, RenderError> {
    let module = format!("{ns}.{}", names::ex_record_name(record));
    for field in &record.fields {
        ty::check_boundary(&field.ty).map_err(|error| {
            RenderError::new(format!(
                "field `{}` of record `{}`: {}",
                field.name, record.name, error.message
            ))
        })?;
        if field.names.ex.is_some() {
            return Err(RenderError::new(format!(
                "field `{}` of record `{}` has an ex rename, but rustler's \
                 NifStruct derives the Elixir keys from the Rust field \
                 names; rename the Rust field instead",
                field.name, record.name
            )));
        }
    }
    let outer: syn::Attribute = parse_quote!(#[derive(::rustler::NifStruct)]);
    let module_attr: syn::Attribute = parse_quote!(#[module = #module]);
    Ok(RenderedRecord {
        outer: vec![outer, module_attr],
        fields: record.fields.iter().map(|_| Vec::new()).collect(),
    })
}
