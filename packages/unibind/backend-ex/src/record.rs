//! Derive `NifStruct` onto record structs.

use syn::parse_quote;
use unibind_core::ir;
use unibind_core::render::{RenderError, RenderedRecord};

use crate::{names, ty};

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
        // Free functions carry binaries through a wire newtype the wrapper
        // converts at the call site (`ty::to_wire`). A record has no such
        // site: `#[unibind::record]` splices `#[derive(NifStruct)]` onto
        // the user's own struct, so the field would encode through
        // rustler's element-wise `Vec<u8>` impl and arrive as a list of
        // integers. Rejected rather than silently mis-encoded.
        if ty::contains_bytes(&field.ty) {
            return Err(RenderError::new(format!(
                "field `{}` of record `{}` carries binary data, which the \
                 elixir backend cannot put on a record: rustler's \
                 `NifStruct` derive reads the field in place and encodes \
                 `Vec<u8>` as a list of integers. Pass the bytes through a \
                 function argument or return value instead, where the \
                 wrapper converts them to a binary.",
                field.name, record.name
            )));
        }
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
